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
    testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt},
};
use ton_connect_bridge_rs::{
    config::Config,
    handlers::SendMessageResponse,
    message_courier::{MessageCourier, RedisMessageCourier},
    server,
    storage::RedisEventStorage,
};

async fn start_test_server() -> (TestServer, ContainerAsync<Redis>) {
    let redis_container = Redis::default().with_tag("alpine").start().await.unwrap();
    let redis_host = redis_container.get_host().await.unwrap();
    let redis_port = redis_container
        .get_host_port_ipv4(REDIS_PORT)
        .await
        .unwrap();
    let redis_url = format!("redis://{redis_host}:{redis_port}");

    let redis_conn_manager = RedisConnectionManager::new(redis_url.clone()).unwrap();
    let redis_pool = bb8::Pool::builder()
        .connection_timeout(Duration::from_secs(3))
        .build(redis_conn_manager)
        .await
        .unwrap();

    let config = Config::new().unwrap();
    let event_storage = RedisEventStorage::new(
        redis_pool,
        config.inbox_inactive_ttl_sec,
        config.inbox_max_messages_per_client,
    );

    let redis_client = redis::Client::open(redis_url.clone()).unwrap();
    let subscription_manager =
        RedisMessageCourier::new(redis_client, config.sse_client_without_messages_ttl_sec);

    let manager = subscription_manager.clone();
    manager.start("messages");

    let app_state = server::AppState {
        config,
        event_saver: Arc::new(event_storage),
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
) -> eventsource_stream::Event {
    let event = event_result.expect("unexpected event error");

    assert_eq!("message", event.event);
    assert!(event.id.len() > 5, "id must be a non-empty string");

    let parsed_data: serde_json::Value =
        serde_json::from_str(&event.data).expect("failed to parse content JSON data");

    let expected_data = json!({
        "from": expected_from,
        "message": BASE64_STANDARD.encode(expected_message),
    });
    assert_eq!(
        expected_data,
        parsed_data,
        "unexpected data content\nexpected message: {},\nactual message: {}",
        expected_message,
        String::from_utf8(
            BASE64_STANDARD
                .decode(parsed_data["message"].as_str().unwrap_or_default())
                .unwrap_or_default()
        )
        .unwrap_or_default(),
    );

    assert!(event.retry.is_none(), "retry field must be empty");
    event
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
    assert_eq!(
        StatusCode::OK,
        response.status(),
        "response body: {}",
        response.text().await.unwrap()
    );
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

#[tokio::test]
async fn test_wallet_reconnection_with_missed_messages() {
    let (server, _redis_container) = start_test_server().await;
    let server_url = server.server_address().unwrap();

    // SSE connection for Wallet
    let mut wallet_stream = must_subscribe_to_events(&server_url, "wallet", None).await;
    // SSE connection for Dapp
    let mut dapp_stream = must_subscribe_to_events(&server_url, "dapp", None).await;

    // Wallet sends a message to Dapp
    let wallet_msg = "Hello from Wallet";
    must_send_message(&server, "wallet", "dapp", &wallet_msg).await;
    // verify Dapp receives the message
    let dapp_chunk = dapp_stream.next().await.unwrap();
    validate_sse_message(dapp_chunk, "wallet", &wallet_msg);

    // Dapp sends a message to Wallet
    let dapp_msg = "Hello from Dapp";
    must_send_message(&server, "dapp", "wallet", &dapp_msg).await;
    // verify Wallet receives the message
    let wallet_chunk = wallet_stream.next().await.unwrap();
    // extract the event id to use on reconnect
    let last_event_id = validate_sse_message(wallet_chunk, "dapp", &dapp_msg).id;

    // Wallet disconnects
    drop(wallet_stream);

    // Dapp sends 5 more messages while Wallet is disconnected
    for i in 0..5 {
        let msg = format!("Offline message {}", i);
        must_send_message(&server, "dapp", "wallet", &msg).await;
    }

    // Wallet reconnects using the last received event id
    let mut reconnected_wallet_stream =
        must_subscribe_to_events(&server_url, "wallet", Some(&last_event_id)).await;
    // verify Wallet receives the 5 missed messages
    for i in 0..5 {
        let chunk = reconnected_wallet_stream.next().await.unwrap();
        let expected_msg = format!("Offline message {}", i);
        validate_sse_message(chunk, "dapp", &expected_msg);
    }

    // verify that there aren't more messages left for the Wallet
    assert!(reconnected_wallet_stream.next().now_or_never().is_none());
}
