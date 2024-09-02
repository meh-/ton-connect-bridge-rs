use axum::http::StatusCode;
use axum_test::{TestServer, TestServerConfig};
use bb8_redis::RedisConnectionManager;
use futures::stream::StreamExt;
use reqwest::Client;
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

#[tokio::test]
async fn test_post_and_receive_single_message() {
    let (server, _redis_container) = start_test_server().await;
    let server_url = server.server_address().unwrap();

    let client = Client::new();
    let events_resp = client
        .get(&format!("{}events?client_id=test_client", server_url))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, events_resp.status());
    let mut events_resp_stream = events_resp.bytes_stream();

    let message = "hello world!";

    let msg_resp = server
        .post("/message?client_id=app&to=test_client")
        .bytes(message.into())
        .await;
    msg_resp.assert_status(StatusCode::OK);
    msg_resp.assert_text(
        serde_json::to_string(&SendMessageResponse {
            message: "OK".to_owned(),
            code: 200,
        })
        .unwrap(),
    );
    // read the SSE stream and verify the message
    let chunk = events_resp_stream.next().await.unwrap();
    let chunk = chunk.unwrap();
    let actual_event = String::from_utf8(chunk.to_vec()).unwrap();

    let actual_event_parts: Vec<&str> = actual_event.split('\n').collect();
    // expected event split line by line:
    // event: message
    // id: <any string>
    // data: {"from":<message author>,"message":<message data>}
    // <empty line>
    // <empty line>
    assert_eq!(5, actual_event_parts.len());
    assert_eq!("event: message", actual_event_parts[0]);
    assert!(actual_event_parts[1].starts_with("id: "));
    assert_eq!(
        "data: {\"from\":\"app\",\"message\":\"hello world!\"}",
        actual_event_parts[2]
    );
    assert_eq!("", actual_event_parts[3]);
    assert_eq!("", actual_event_parts[4]);
}
