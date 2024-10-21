use super::{AppError, Query, ValidationError, MAX_CLIENT_ID_LEN};
use crate::server::AppState;
use crate::storage::EventStorage;
use crate::{message_courier::MessageCourier, models::TonEvent};
use axum::{
    extract::State,
    response::sse::{self, Event, Sse},
};
use futures::stream::{select_all, Stream};
use serde::{Deserialize, Deserializer};
use std::pin::Pin;
use std::task::Poll;
use std::time::Instant;
use std::{convert::Infallible, time::Duration};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt as _;

pub async fn sse_handler<S, C>(
    Query(params): Query<SubscribeToEventsQueryParams>,
    State(state): State<AppState<S, C>>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError>
where
    S: EventStorage,
    C: MessageCourier,
{
    params.validate(state.config.max_client_ids_per_connection)?;

    metrics::histogram!("client_ids_per_connection").record(params.client_ids.len() as f64);

    let mut old_events_streams = vec![];
    if let Some(last_event_id) = params.last_event_id {
        for client_id in &params.client_ids {
            let events = state
                .event_saver
                .get_since(&client_id, &last_event_id)
                .await?;
            let stream = tokio_stream::iter(
                events
                    .into_iter()
                    .map(|ton_event| Ok(EventResponse::Message(ton_event).into())),
            );
            old_events_streams.push(stream);
        }
    }
    let combined_old_events_stream = select_all(old_events_streams);

    let mut live_streams = vec![];
    for client_id in params.client_ids {
        let rx = state.subscription_manager.register_client(client_id);
        let stream = UnboundedReceiverStream::new(rx)
            .map(|ton_event| Ok(EventResponse::Message(ton_event).into()));
        live_streams.push(stream);
    }
    let combined_live_stream = select_all(live_streams);

    let stream = combined_old_events_stream.chain(combined_live_stream);
    Ok(Sse::new(TrackedStream::new(stream)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(state.config.sse_heartbeat_interval_sec))
            .event(EventResponse::Heartbeat.into()),
    ))
}

#[derive(Deserialize, Debug)]
pub struct SubscribeToEventsQueryParams {
    #[serde(rename = "client_id", deserialize_with = "deserialize_list_of_strings")]
    client_ids: Vec<String>,
    last_event_id: Option<String>,
}

impl SubscribeToEventsQueryParams {
    fn validate(&self, max_client_ids: usize) -> Result<(), ValidationError> {
        if self.client_ids.is_empty() {
            return Err(ValidationError(
                "Failed to deserialize query string: field `client_id` is empty".into(),
            ));
        }

        if self.client_ids.len() > max_client_ids {
            return Err(ValidationError(
                format!("Failed to deserialize query string: field `client_id` must contain no more than {max_client_ids} ids"),
            ));
        }

        for id in &self.client_ids {
            if id.len() > MAX_CLIENT_ID_LEN {
                return Err(ValidationError(
                    format!("Failed to deserialize query string: field `client_id` must not exceed {MAX_CLIENT_ID_LEN} characters"),
                ));
            }
        }

        Ok(())
    }
}

fn deserialize_list_of_strings<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let r = s
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    Ok(r)
}

pub enum EventResponse {
    Heartbeat,
    Message(TonEvent),
}

impl From<EventResponse> for sse::Event {
    fn from(ev_resp: EventResponse) -> Self {
        match ev_resp {
            EventResponse::Heartbeat => sse::Event::default().event("heartbeat"),
            EventResponse::Message(ton_event) => {
                let data = ton_event.serialize_for_sse().unwrap();
                sse::Event::default()
                    .event("message")
                    .id(ton_event.id)
                    .data(data)
            }
        }
    }
}

struct TrackedStream<S> {
    start: Instant,
    inner: S,
}

impl<S> TrackedStream<S> {
    pub fn new(inner: S) -> Self {
        metrics::gauge!("active_connections_number").increment(1);
        Self {
            start: Instant::now(),
            inner,
        }
    }
}

impl<S> Stream for TrackedStream<S>
where
    S: Stream + Unpin,
{
    type Item = S::Item;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(item)) => {
                metrics::counter!("messages_sent_to_clients_total").increment(1);
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for TrackedStream<S> {
    fn drop(&mut self) {
        let duration = self.start.elapsed().as_secs_f64();
        metrics::histogram!("sse_session_duration_seconds").record(duration);
        metrics::gauge!("active_connections_number").decrement(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        mocks::{MockCourier, MockStorage},
    };
    use axum::{http::Request, routing::get, Router};
    use reqwest::StatusCode;
    use serde_json::json;
    use std::sync::Arc;
    use tower::util::ServiceExt;

    const EP_PATH: &str = "/events";
    const MAX_CLIENT_IDS: usize = 10;

    async fn exec(
        query_string: String,
        storage: Option<MockStorage>,
        courier: Option<MockCourier>,
    ) -> (StatusCode, serde_json::Value) {
        let mut config = Config::new().unwrap();
        config.max_client_ids_per_connection = MAX_CLIENT_IDS;

        let app_state = AppState {
            config,
            event_saver: Arc::new(storage.unwrap_or(MockStorage::new())),
            subscription_manager: Arc::new(courier.unwrap_or(MockCourier::new())),
        };

        let app = Router::new()
            .route(EP_PATH, get(sse_handler))
            .with_state(app_state);

        let request = Request::builder()
            .uri(EP_PATH.to_string() + "?" + &query_string)
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(request).await.unwrap();
        let code = resp.status();
        let resp_body = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();

        let resp_json: serde_json::Value = serde_json::from_str(&resp_body).unwrap();
        (code, resp_json)
    }

    #[tokio::test]
    async fn test_missing_client_id() {
        let expected_resp = json!(AppError {
            message: "Failed to deserialize query string: missing field `client_id`".to_string(),
            status: StatusCode::BAD_REQUEST,
        });

        let q_string = "cli=1".to_string();

        let (status, resp_body) = exec(q_string, None, None).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_empty_client_id() {
        let expected_resp = json!(AppError {
            message: "Failed to deserialize query string: field `client_id` is empty".to_string(),
            status: StatusCode::BAD_REQUEST,
        });

        let q_string = "client_id=&last_event_id=2".to_string();

        let (status, resp_body) = exec(q_string, None, None).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_to_many_client_ids() {
        let expected_resp = json!(AppError {
            message: format!("Failed to deserialize query string: field `client_id` must contain no more than {MAX_CLIENT_IDS} ids"),
            status: StatusCode::BAD_REQUEST,
        });

        let ids = (1..MAX_CLIENT_IDS * 2)
            .map(|i| i.to_string())
            .collect::<Vec<String>>()
            .join(",");
        let q_string = format!("client_id={ids}");
        let (status, resp_body) = exec(q_string, None, None).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_too_long_client_id_value() {
        let expected_resp = json!(AppError {
            message: format!("Failed to deserialize query string: field `client_id` must not exceed {MAX_CLIENT_ID_LEN} characters"),
            status: StatusCode::BAD_REQUEST,
        });

        let long_id = "a".repeat(MAX_CLIENT_ID_LEN + 1);
        let q_string = format!("client_id=1,{long_id}");
        let (status, resp_body) = exec(q_string, None, None).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }
}
