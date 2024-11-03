use crate::models::TonEvent;
use anyhow::{bail, Result};
use bb8_redis::RedisConnectionManager;
use redis::{
    streams::{StreamReadOptions, StreamReadReply},
    AsyncCommands,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::mpsc::{self};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Duration;

pub trait MessageCourier: Send + Sync {
    fn register_client(&self, client_id: String) -> UnboundedReceiver<TonEvent>;
    fn start(self, channel: &str);
}

#[derive(Clone)]
pub struct RedisMessageCourier {
    clients: Arc<Mutex<HashMap<String, ClientSubscription>>>,
    redis_pool: Arc<bb8::Pool<RedisConnectionManager>>,
    client_without_messages_ttl_sec: Duration,
}

struct ClientSubscription {
    tx: UnboundedSender<TonEvent>,
    last_message_at: Instant,
}

impl RedisMessageCourier {
    pub fn new(
        redis_pool: bb8::Pool<RedisConnectionManager>,
        client_without_messages_ttl_sec: u64,
    ) -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            redis_pool: Arc::new(redis_pool),
            client_without_messages_ttl_sec: Duration::from_secs(client_without_messages_ttl_sec),
        }
    }

    fn cleanup_inactive_clients(&self) {
        let clients = Arc::clone(&self.clients);
        let ttl = self.client_without_messages_ttl_sec.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(ttl);
            loop {
                interval.tick().await;
                let mut cli = clients.lock().unwrap();
                metrics::gauge!("active_tracked_client_ids").set(cli.len() as f64);
                let now = Instant::now();
                cli.retain(|client_id, sub| {
                    if now.duration_since(sub.last_message_at) < ttl {
                        true
                    } else {
                        tracing::debug!("removing inactive client {client_id}");
                        false
                    }
                });
            }
        });
    }

    async fn process_stream(&self, channel: &str) {
        let mut last_event_id = "$".to_string();
        let read_opts = StreamReadOptions::default().block(1000).count(500);

        loop {
            let mut conn = match self.redis_pool.get().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::error!("getting redis connection from pool: {}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            loop {
                tracing::debug!("executing xread with id {}", &last_event_id);
                let reply: StreamReadReply = match conn
                    .xread_options(&[channel], &[&last_event_id], &read_opts)
                    .await
                {
                    Ok(reply) => reply,
                    Err(e) => {
                        tracing::error!("executing XREAD command: {}", e);
                        break;
                    }
                };

                for stream_res in reply.keys {
                    for entry in stream_res.ids {
                        if let Err(e) = self.process_stream_event(&entry).await {
                            tracing::error!("processing live stream entry: {}", e);
                            metrics::counter!("invalid_stream_events_total").increment(1);
                        }
                        last_event_id = entry.id.clone();
                    }
                }
            }
        }
    }

    async fn process_stream_event(&self, entry: &redis::streams::StreamId) -> Result<()> {
        if let Some(redis::Value::Data(raw_event)) = entry.get("event") {
            let payload = String::from_utf8(raw_event.to_vec())?;
            tracing::debug!("live stream event: {}", payload);
            let event: TonEvent = serde_json::from_str(&payload)?;

            let mut clients = self.clients.lock().unwrap();
            if let Some(sub) = clients.get_mut(&event.to.clone()) {
                tracing::debug!("sending message to '{}'", event.to);
                if let Err(e) = sub.tx.send(event.clone()) {
                    tracing::debug!("error sending message to '{}': {}", event.to, e);
                    clients.remove(&event.to);
                } else {
                    sub.last_message_at = Instant::now();
                }
            } else {
                tracing::debug!("no client found for '{}'", event.to);
            }
            Ok(())
        } else {
            bail!("live stream entry doesn't contain event field");
        }
    }
}

impl MessageCourier for RedisMessageCourier {
    fn start(self, channel: &str) {
        self.cleanup_inactive_clients();

        let channel = channel.to_string();
        tokio::spawn(async move {
            self.process_stream(&channel).await;
        });
    }

    fn register_client(&self, client_id: String) -> UnboundedReceiver<TonEvent> {
        let (tx, rx) = mpsc::unbounded_channel::<TonEvent>();
        let sub = ClientSubscription {
            tx: tx.clone(),
            last_message_at: Instant::now(),
        };
        self.clients.lock().unwrap().insert(client_id, sub);
        rx
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use bb8_redis::RedisConnectionManager;
    use redis::AsyncCommands;
    use std::time::{SystemTime, UNIX_EPOCH};
    use testcontainers_modules::{
        redis::{Redis, REDIS_PORT},
        testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt},
    };
    use tokio::time::timeout;

    const MESSAGES_STREAM: &str = "messages";

    async fn setup_redis() -> (bb8::Pool<RedisConnectionManager>, ContainerAsync<Redis>) {
        let container = Redis::default().with_tag("alpine").start().await.unwrap();
        let host = container.get_host().await.unwrap();
        let port = container.get_host_port_ipv4(REDIS_PORT).await.unwrap();
        let conn_str = format!("redis://{host}:{port}");
        let manager = RedisConnectionManager::new(conn_str).unwrap();
        let pool = bb8::Pool::builder().build(manager).await.unwrap();
        (pool, container)
    }

    #[tokio::test]
    async fn test_inactive_client_is_removed() {
        let (redis_pool, _container) = setup_redis().await;
        let inactive_ttl: Duration = Duration::from_secs(1);
        let courier = RedisMessageCourier::new(redis_pool.clone(), inactive_ttl.as_secs());

        let client_id = "wallet_client".to_string();

        // start the message courier and subscribe the client
        courier.clone().start(MESSAGES_STREAM);
        let mut rx = courier.register_client(client_id.clone());
        // let the courier spawn all tasks
        tokio::time::sleep(Duration::from_millis(100)).await;

        // send a dummy ton event to the subscribed client via a real redis connection
        let event = TonEvent {
            id: "123".to_string(),
            from: "dapp".to_string(),
            to: client_id.clone(),
            message: "hello".to_string(),
            deadline: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 10,
        };
        let value = serde_json::to_string(&event).expect("marshalling test ton event");
        let _: () = redis_pool
            .get()
            .await
            .unwrap()
            .xadd(MESSAGES_STREAM.to_string(), "*", &[("event", value)])
            .await
            .expect("sending test message to stream");

        // verify that the client receives the ton event as is
        match timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some(received_event)) => assert_eq!(event, received_event),
            Ok(None) => panic!("channel closed unexpectedly"),
            Err(_) => panic!("did not receive the expected event within specified timeout"),
        }

        // wait while the subscribed client becomes inactive
        tokio::time::sleep(2 * inactive_ttl).await;
        // verify that the client id is removed from the subscribed clients due to inactivity
        {
            let active_clients = courier.clients.lock().unwrap();
            assert!(
                !active_clients.contains_key(&client_id),
                "inactive client wasn't removed"
            )
        }
    }
}
