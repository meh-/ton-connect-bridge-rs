use crate::models::TonEvent;
use bb8_redis::RedisConnectionManager;
use redis::streams::{StreamId, StreamRangeReply};
use redis::{streams::StreamMaxlen, AsyncCommands, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const ALL_MSGS_CHAN: &str = "messages";

#[derive(Debug)]
pub struct EventStorageError(String);

impl std::error::Error for EventStorageError {}

impl std::fmt::Display for EventStorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub trait EventStorage: Send + Sync {
    fn add(
        &self,
        event: TonEvent,
    ) -> impl std::future::Future<Output = Result<(), EventStorageError>> + Send;
    fn get_since(
        &self,
        client_id: &String,
        event_id: &String,
    ) -> impl std::future::Future<Output = Result<Vec<TonEvent>, EventStorageError>> + Send;
}

#[derive(Debug, Clone)]
pub struct RedisEventStorage {
    pub redis_pool: Arc<bb8::Pool<RedisConnectionManager>>,
    inbox_inactive_ttl_sec: i64,
    max_messages_per_inbox: usize,
}

impl RedisEventStorage {
    pub fn new(
        redis_pool: bb8::Pool<RedisConnectionManager>,
        inbox_inactive_ttl_sec: u16,
        max_messages_per_inbox: usize,
    ) -> Self {
        Self {
            redis_pool: Arc::new(redis_pool),
            inbox_inactive_ttl_sec: inbox_inactive_ttl_sec.into(),
            max_messages_per_inbox,
        }
    }
}

impl EventStorage for RedisEventStorage {
    async fn add(&self, mut event: TonEvent) -> Result<(), EventStorageError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|err| EventStorageError(err.to_string()))?;

        let inbox_key = format!("inbox:{}", event.to);
        let value =
            serde_json::to_string(&event).map_err(|err| EventStorageError(err.to_string()))?;

        let results: [String; 1] = redis::pipe()
            .xadd_maxlen(
                inbox_key.clone(),
                StreamMaxlen::Approx(self.max_messages_per_inbox),
                "*",
                &[("event", value.clone())],
            )
            .expire(inbox_key, self.inbox_inactive_ttl_sec)
            .ignore()
            .query_async(&mut *conn)
            .await
            .map_err(|err| EventStorageError(err.to_string()))?;

        event.id = results[0].clone();

        let value =
            serde_json::to_string(&event).map_err(|err| EventStorageError(err.to_string()))?;
        let _: () = conn
            .publish(ALL_MSGS_CHAN, value)
            .await
            .map_err(|err| EventStorageError(err.to_string()))?;

        Ok(())
    }

    async fn get_since(
        &self,
        client_id: &String,
        event_id: &String,
    ) -> Result<Vec<TonEvent>, EventStorageError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|err| EventStorageError(err.to_string()))?;

        let inbox_key = format!("inbox:{}", client_id);

        let mut events = Vec::new();

        let srr: StreamRangeReply = conn
            // prefix the event_id with '\(' to get all entries after that id
            .xrange(inbox_key, format!("({}", event_id), "+")
            .await
            .map_err(|err| EventStorageError(err.to_string()))?;

        let cur_timestmap = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        for StreamId { id, map } in srr.ids {
            if let Some(Value::Data(raw_event)) = map.get("event") {
                let mut event: TonEvent = serde_json::from_str(
                    String::from_utf8(raw_event.to_vec())
                        .expect("utf8")
                        .as_str(),
                )
                .map_err(|err| EventStorageError(err.to_string()))?;

                // do not return expired events
                if cur_timestmap > event.deadline {
                    continue;
                }
                event.id = id.to_string();
                events.push(event);
            } else {
                return Err(EventStorageError("unexpected event data".into()));
            }
        }

        Ok(events)
    }
}
