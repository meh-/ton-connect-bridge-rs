use crate::models::TonEvent;
use serde_json::from_str;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::mpsc::{self};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Duration;
use tokio_stream::StreamExt;

pub trait MessageCourier: Send + Sync {
    fn register_client(&self, client_id: String) -> UnboundedReceiver<TonEvent>;
    fn start(self, channel: &str);
}

#[derive(Clone)]
pub struct RedisMessageCourier {
    clients: Arc<Mutex<HashMap<String, ClientSubscription>>>,
    redis: Arc<redis::Client>,
    client_without_messages_ttl_sec: Duration,
}

struct ClientSubscription {
    tx: UnboundedSender<TonEvent>,
    last_message_at: Instant,
}

impl RedisMessageCourier {
    pub fn new(redis: redis::Client, client_without_messages_ttl_sec: u64) -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            redis: Arc::new(redis),
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
}

impl MessageCourier for RedisMessageCourier {
    fn start(self, channel: &str) {
        self.cleanup_inactive_clients();

        let channel = channel.to_string();
        tokio::spawn(async move {
            let mut pubsub = self.redis.get_async_pubsub().await.unwrap();
            pubsub.subscribe(channel).await.unwrap();

            while let Some(msg) = pubsub.on_message().next().await {
                let payload: String = msg.get_payload().unwrap();
                tracing::debug!("channel '{}': {}", msg.get_channel_name(), payload);

                if let Ok(event) = from_str::<TonEvent>(&payload) {
                    let mut cli = self.clients.lock().unwrap();
                    if let Some(sub) = cli.get_mut(&event.to.clone()) {
                        tracing::debug!("sending message to '{}'", event.to);
                        if let Err(e) = sub.tx.send(event.clone()) {
                            tracing::debug!("error sending message to '{}': {}", event.to, e);
                            cli.remove(&event.to);
                        } else {
                            sub.last_message_at = Instant::now();
                        }
                    } else {
                        tracing::info!("no client found for '{}'", event.to);
                    }
                } else {
                    tracing::error!("invalid message: '{}'", payload);
                }
            }
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
    use redis::Commands;
    use std::time::{SystemTime, UNIX_EPOCH};
    use testcontainers_modules::{
        redis::{Redis, REDIS_PORT},
        testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt},
    };
    use tokio::time::timeout;

    const MESSAGES_CHANNEL: &str = "messages";

    async fn setup_redis() -> (redis::Client, ContainerAsync<Redis>) {
        let container = Redis::default().with_tag("alpine").start().await.unwrap();
        let host = container.get_host().await.unwrap();
        let port = container.get_host_port_ipv4(REDIS_PORT).await.unwrap();
        let conn_str = format!("redis://{host}:{port}");
        let redis_client = redis::Client::open(conn_str.clone()).unwrap();
        (redis_client, container)
    }

    #[tokio::test]
    async fn test_inactive_client_is_removed() {
        let (redis_client, _container) = setup_redis().await;
        let inactive_ttl: Duration = Duration::from_secs(1);
        let courier = RedisMessageCourier::new(redis_client.clone(), inactive_ttl.as_secs());

        let client_id = "wallet_client".to_string();

        // start the message courier and subscribe the client
        courier.clone().start(MESSAGES_CHANNEL);
        let mut rx = courier.register_client(client_id.clone());
        // let the courier to spawn all tasks
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
        redis_client
            .get_connection()
            .unwrap()
            .publish::<_, _, ()>(MESSAGES_CHANNEL, value)
            .expect("sending test message to pubsub");

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
