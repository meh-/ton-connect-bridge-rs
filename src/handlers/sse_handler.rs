use super::{AppError, Query};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt as _;

use crate::models::TonEvent;
use crate::server::AppState;
use crate::storage::EventStorage;
use axum::{
    extract::State,
    response::sse::{self, Event, Sse},
};
use futures::stream::{select_all, Stream};
use serde::{Deserialize, Deserializer};
use std::{convert::Infallible, time::Duration};

const HEARTBEAT_INTERVAL_SECS: u64 = 5;

pub async fn sse_handler(
    Query(params): Query<SubscribeToEventsQueryParams>,
    State(state): State<AppState>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
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
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS))
            .event(EventResponse::Heartbeat.into()),
    ))
}

#[derive(Deserialize)]
pub struct SubscribeToEventsQueryParams {
    #[serde(rename = "client_id", deserialize_with = "deserialize_list_of_strings")]
    client_ids: Vec<String>,
    last_event_id: Option<String>,
}

fn deserialize_list_of_strings<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(s.split(',').map(str::to_string).collect())
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
