use bb8_redis::RedisConnectionManager;

use redis::AsyncCommands;
use std::sync::Arc;
use std::time::Duration;
use ton_connect_bridge_rs::message_courier::{MessageCourier, RedisMessageCourier};
use ton_connect_bridge_rs::storage::RedisEventStorage;
use ton_connect_bridge_rs::{config, server};
use tracing::Level;

#[tokio::main]
async fn main() {
    let cfg = config::Config::new().expect("loading app config");
    // to pass the cfg later to the shared app state
    let cfg_clone = cfg.clone();

    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .with_max_level(Level::DEBUG)
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

    let redis_client = redis::Client::open(cfg.redis_url).expect("initializing redis client");
    let subscription_manager =
        RedisMessageCourier::new(redis_client, cfg.sse_client_without_messages_ttl_sec);

    let manager = subscription_manager.clone();
    manager.start("messages");

    let event_storage = RedisEventStorage::new(
        redis_pool,
        cfg.inbox_inactive_ttl_sec,
        cfg.inbox_max_messages_per_client,
    );
    let app_state = server::AppState {
        config: cfg_clone,
        event_saver: Arc::new(event_storage),
        subscription_manager: Arc::new(subscription_manager),
    };
    let router = server::router(app_state);
    server::start(router, cfg.server_address).await;
}
