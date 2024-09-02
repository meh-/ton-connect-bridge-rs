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
    let cfg = config::Config::new().unwrap();

    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .with_max_level(Level::DEBUG)
        .init();

    let redis_conn_manager = RedisConnectionManager::new(cfg.redis_url.clone()).unwrap();
    let redis_pool = Arc::new(
        bb8::Pool::builder()
            .connection_timeout(Duration::from_secs(cfg.redis_conn_timeout_sec))
            .build(redis_conn_manager)
            .await
            .unwrap(),
    );

    {
        // ping redis before starting
        let mut conn = redis_pool.get().await.unwrap();
        conn.set::<&str, &str, ()>("foo", "bar").await.unwrap();
        let result: String = conn.get("foo").await.unwrap();
        assert_eq!(result, "bar");
    }

    let redis_client = redis::Client::open(cfg.redis_url).unwrap();
    let subscription_manager = RedisMessageCourier::new(redis_client);

    let manager = subscription_manager.clone();
    manager.start("messages");

    let app_state = server::AppState {
        event_saver: Arc::new(RedisEventStorage { redis_pool }),
        subscription_manager: Arc::new(subscription_manager),
    };
    let router = server::router(app_state);
    server::start(router, cfg.server_port).await;
}
