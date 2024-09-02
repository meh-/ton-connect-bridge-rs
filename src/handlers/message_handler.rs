use crate::models::TonEvent;
use crate::server::AppState;
use crate::storage::EventStorage;
use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{AppError, Query};

const MAX_TTL_SECS: u16 = 500;
const MIN_ALLOWED_TTL_SECS: u16 = 10;

pub async fn message_handler<S, C>(
    Query(query): Query<SendMessageQueryParams>,
    State(state): State<AppState<S, C>>,
    body: String,
) -> Result<Json<SendMessageResponse>, AppError>
where
    S: EventStorage,
{
    let from = query.client_id;
    let to = query.to;

    let ttl = query
        .ttl
        .map(|v| {
            if v >= MIN_ALLOWED_TTL_SECS {
                v
            } else {
                MAX_TTL_SECS
            }
        })
        .unwrap_or(MAX_TTL_SECS);

    let deadline = SystemTime::now() + Duration::from_secs(ttl.into());
    let event = TonEvent {
        id: "".to_string(),
        from,
        to,
        message: body,
        deadline: deadline.duration_since(UNIX_EPOCH).unwrap().as_secs(),
    };

    state.event_saver.add(event).await?;

    Ok(Json(SendMessageResponse {
        code: StatusCode::OK.into(),
        message: "OK".to_owned(),
    }))
}

#[derive(Deserialize)]
pub struct SendMessageQueryParams {
    client_id: String,
    to: String,
    ttl: Option<u16>,
}

#[derive(Serialize)]
pub struct SendMessageResponse {
    pub message: String,
    #[serde(rename = "statusCode")]
    pub code: u16,
}
