use bb8_redis::RedisConnectionManager;

use redis::AsyncCommands;
use std::sync::Arc;
use std::time::Duration;
use ton_connect_bridge_rs::message_courier::{MessageCourier, RedisMessageCourier};
use ton_connect_bridge_rs::storage::RedisEventStorage;
use ton_connect_bridge_rs::{config, server};

#[tokio::main]
async fn main() {
    let cfg = config::Config::new().expect("loading app config");

    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .with_max_level(cfg.log_level())
        .init();

    let redis_conn_manager = RedisConnectionManager::new(cfg.redis_url.clone())
        .expect("initializing redis connection manager");

    let redis_pool = bb8::Pool::builder()
        .connection_timeout(Duration::from_secs(cfg.redis_conn_timeout_sec))
        .build(redis_conn_manager)
        .await
        .expect("building redis pool");

    {
        // ping redis before starting
        let mut conn = redis_pool
            .get()
            .await
            .expect("getting redis connection from pool");
        conn.set::<&str, &str, ()>("foo", "bar").await.unwrap();
        let result: String = conn.get("foo").await.unwrap();
        assert_eq!(result, "bar");
    }

    let subscription_manager =
        RedisMessageCourier::new(redis_pool.clone(), cfg.sse_client_without_messages_ttl_sec);

    let manager = subscription_manager.clone();
    manager.start("messages");

    let event_storage = RedisEventStorage::new(
        redis_pool,
        cfg.inbox_inactive_ttl_sec,
        cfg.inbox_max_messages_per_client,
        cfg.global_stream_max_size,
    );
    let app_state = server::AppState {
        config: cfg.clone(),
        event_saver: Arc::new(event_storage),
        subscription_manager: Arc::new(subscription_manager),
    };
    let app_router = server::router(app_state);
    let metrics_router = server::metrics_router();

    tokio::join!(
        server::start(app_router, cfg.server_address),
        server::start(metrics_router, cfg.metrics_server_address),
    );
}
