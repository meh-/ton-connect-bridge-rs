use crate::models::TonEvent;
use serde_json::from_str;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc::{self};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_stream::StreamExt;

#[derive(Clone)]
pub struct MessageCourier {
    clients: Arc<Mutex<HashMap<String, UnboundedSender<TonEvent>>>>,
    redis: Arc<redis::Client>,
}

impl MessageCourier {
    pub fn new(redis: redis::Client) -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            redis: Arc::new(redis),
        }
    }

    pub fn start(self, channel: &str) {
        let channel = channel.to_string();

        tokio::spawn(async move {
            let mut pubsub = self.redis.get_async_pubsub().await.unwrap();
            pubsub.subscribe(channel).await.unwrap();

            while let Some(msg) = pubsub.on_message().next().await {
                let payload: String = msg.get_payload().unwrap();
                tracing::debug!("channel '{}': {}", msg.get_channel_name(), payload);

                if let Ok(event) = from_str::<TonEvent>(&payload) {
                    let mut cli = self.clients.lock().unwrap();
                    if let Some(tx) = cli.get(&event.to.clone()) {
                        tracing::debug!("sending message to '{}'", event.to);
                        if let Err(e) = tx.send(event.clone()) {
                            tracing::debug!("error sending message to '{}': {}", event.to, e);
                            cli.remove(&event.to);
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

    pub fn register_client(&self, client_id: String) -> UnboundedReceiver<TonEvent> {
        let (tx, rx) = mpsc::unbounded_channel::<TonEvent>();
        self.clients.lock().unwrap().insert(client_id, tx.clone());
        rx
    }
}
