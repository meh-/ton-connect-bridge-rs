mod message_handler;
mod sse_handler;

pub use message_handler::*;
pub use sse_handler::*;

use crate::storage::EventStorageError;
use axum::{
    extract::{rejection::QueryRejection, FromRequestParts},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Serialize, Serializer};

#[derive(Serialize, Debug)]
pub struct AppError {
    pub message: String,
    #[serde(rename = "statusCode", serialize_with = "serialize_status")]
    pub status: StatusCode,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (self.status, axum::Json(self)).into_response()
    }
}

fn serialize_status<S>(status: &StatusCode, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u16(status.as_u16())
}

impl From<QueryRejection> for AppError {
    fn from(rejection: QueryRejection) -> Self {
        Self {
            status: rejection.status(),
            message: rejection.body_text(),
        }
    }
}

impl From<EventStorageError> for AppError {
    fn from(e: EventStorageError) -> Self {
        tracing::error!("unexpected EventStorageError: {e}");

        AppError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "something went wrong".into(),
        }
    }
}

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppError))]
pub struct Query<T>(T);
