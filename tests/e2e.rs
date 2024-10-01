use axum::http::StatusCode;
use axum_test::{TestServer, TestServerConfig};
use base64::prelude::*;
use bb8_redis::RedisConnectionManager;
use eventsource_stream::Eventsource;
use futures::{stream::StreamExt, FutureExt, Stream};
use reqwest::{Client, Url};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use testcontainers_modules::{
    redis::{Redis, REDIS_PORT},
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use ton_connect_bridge_rs::{
    handlers::SendMessageResponse,
    message_courier::{MessageCourier, RedisMessageCourier},
    server,
    storage::RedisEventStorage,
};

async fn start_test_server() -> (TestServer, ContainerAsync<Redis>) {
    let redis_container = Redis::default().start().await.unwrap();
    let host_ip = redis_container.get_host().await.unwrap();
    let host_port = redis_container
        .get_host_port_ipv4(REDIS_PORT)
        .await
        .unwrap();
    let redis_url = format!("redis://{host_ip}:{host_port}");

    let redis_conn_manager = RedisConnectionManager::new(redis_url.clone()).unwrap();
    let redis_pool = Arc::new(
        bb8::Pool::builder()
            .connection_timeout(Duration::from_secs(3))
            .build(redis_conn_manager)
            .await
            .unwrap(),
    );

    let redis_client = redis::Client::open(redis_url).unwrap();
    let subscription_manager = RedisMessageCourier::new(redis_client);

    let manager = subscription_manager.clone();
    manager.start("messages");

    let app_state = server::AppState {
        event_saver: Arc::new(RedisEventStorage { redis_pool }),
        subscription_manager: Arc::new(subscription_manager),
    };

    let app = server::router(app_state);
    let config = TestServerConfig::builder().http_transport().build();

    (
        TestServer::new_with_config(app, config).unwrap(),
        redis_container,
    )
}

// expected SSE event line by line:
// event: message
// id: <any string>
// data: {"from":<message author>,"message":<message data>}
// <empty line>
// <empty line>
fn validate_sse_message(
    event_result: Result<
        eventsource_stream::Event,
        eventsource_stream::EventStreamError<reqwest::Error>,
    >,
    expected_from: &str,
    expected_message: &str,
) {
    let event = event_result.expect("unexpected event error");

    assert_eq!("message", event.event);
    assert!(event.id.len() > 5, "id must be a non-empty string");

    let parsed_data: serde_json::Value =
        serde_json::from_str(&event.data).expect("failed to parse content JSON data");

    let expected_data = json!({
        "from": expected_from,
        "message": BASE64_STANDARD.encode(expected_message),
    });
    assert_eq!(expected_data, parsed_data, "unexpected message data");

    assert!(event.retry.is_none(), "retry field must be empty");
}

async fn must_send_message(server: &TestServer, from: &str, to: &str, message: &str) {
    let resp = server
        .post(&format!("/message?client_id={}&to={}", from, to))
        .bytes(BASE64_STANDARD.encode(message).into())
        .await;

    resp.assert_status(StatusCode::OK);
    resp.assert_text(
        serde_json::to_string(&SendMessageResponse {
            message: "OK".to_owned(),
            code: 200,
        })
        .unwrap(),
    );
}

async fn must_subscribe_to_events(
    server_url: &Url,
    client_id: &str,
    last_event_id: Option<&str>,
) -> impl Stream<
    Item = Result<eventsource_stream::Event, eventsource_stream::EventStreamError<reqwest::Error>>,
> {
    let mut url = format!("{}events?client_id={}", server_url, client_id);

    if let Some(event_id) = last_event_id {
        url.push_str(&format!("&last_event_id={}", event_id));
    }

    let response = Client::new().get(&url).send().await.unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!(
        "text/event-stream",
        response.headers()[reqwest::header::CONTENT_TYPE],
        "incorrect content-type header"
    );
    assert_eq!(
        "no-cache",
        response.headers()[reqwest::header::CACHE_CONTROL],
        "incorrect cache-control header"
    );

    response.bytes_stream().eventsource()
}

#[tokio::test]
async fn test_post_and_receive_single_message() {
    let (server, _redis_container) = start_test_server().await;
    let server_url = server.server_address().unwrap();

    let client_id = "test_client";
    let mut events_resp_stream = must_subscribe_to_events(&server_url, &client_id, None).await;

    let message = "hello world!";
    must_send_message(&server, "app", &client_id, &message).await;

    let chunk = events_resp_stream.next().await.unwrap();
    validate_sse_message(chunk, "app", &message);
}

#[tokio::test]
async fn test_multiple_clients_communication() {
    let (server, _redis_container) = start_test_server().await;
    let server_url = server.server_address().unwrap();

    // SSE connections for N Wallets
    const NUM_WALLETS: usize = 20;
    let mut wallet_streams = Vec::with_capacity(NUM_WALLETS);
    for w_idx in 0..NUM_WALLETS {
        let w_stream =
            must_subscribe_to_events(&server_url, &format!("wallet_{}", w_idx), None).await;
        wallet_streams.push(w_stream);
    }

    // SSE connection for Dapp
    let mut dapp_stream = must_subscribe_to_events(&server_url, "dapp", None).await;

    const NUM_MESSAGE_CYCLES: usize = 5;
    for cycle_idx in 0..NUM_MESSAGE_CYCLES {
        // each Wallet sends a message to Dapp
        for w_idx in 0..NUM_WALLETS {
            let wallet_msg = format!("[cycle{}] Hello from Wallet{}", cycle_idx, w_idx);
            must_send_message(&server, &format!("wallet_{}", w_idx), "dapp", &wallet_msg).await;

            // verify Dapp receives the message from the wallet
            let dapp_chunk = dapp_stream.next().await.unwrap();
            validate_sse_message(dapp_chunk, &format!("wallet_{w_idx}"), &wallet_msg);
        }

        // Dapp replies to each wallet
        for w_idx in 0..NUM_WALLETS {
            let dapp_reply = format!("Reply to Wallet{}", w_idx);
            must_send_message(&server, "dapp", &format!("wallet_{}", w_idx), &dapp_reply).await;

            // verify the wallet receives the reply
            let wallet_chunk = wallet_streams[w_idx].next().await.unwrap();
            validate_sse_message(wallet_chunk, "dapp", &dapp_reply);
        }
    }
    // verify that there aren't more messages left for the Wallets
    for (i, mut stream) in wallet_streams.into_iter().enumerate() {
        assert!(
            stream.next().now_or_never().is_none(),
            "wallet_{} received an unexpected message",
            i
        );
    }
}
