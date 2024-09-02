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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{message_courier::MessageCourier, storage::EventStorageError};
    use axum::{http::Request, routing::post, Router};
    use mockall::mock;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tower::util::ServiceExt;

    mock! {
        #[derive(Clone)]
        Storage {}
        impl EventStorage for Storage {
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
        impl Clone for Storage {
            fn clone(&self) -> Self;
        }
    }

    mock! {
        #[derive(Clone)]
        Courier {}
        impl MessageCourier for Courier {
            fn register_client(&self, client_id: String) -> UnboundedReceiver<TonEvent>;
            fn start(self, channel: &str);
        }
        impl Clone for Courier {
            fn clone(&self) -> Self;
        }
    }

    const EP_PATH: &str = "/message";

    fn create_app<S, C>(app_state: AppState<S, C>) -> Router
    where
        S: EventStorage + Clone + Send + Sync + 'static,
        C: MessageCourier + Clone + Send + Sync + 'static,
    {
        Router::new()
            .route(EP_PATH, post(message_handler))
            .with_state(app_state)
    }

    async fn exec(
        storage: MockStorage,
        query_string: String,
        req_body: axum::body::Body,
    ) -> (StatusCode, serde_json::Value) {
        let courier_mock = MockCourier::new();
        let app_state = AppState {
            event_saver: Arc::new(storage),
            subscription_manager: Arc::new(courier_mock),
        };

        let app = create_app(app_state);
        let request = Request::builder()
            .method("POST")
            .uri(EP_PATH.to_string() + "?" + &query_string)
            .body(req_body)
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
    async fn test_missing_to() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: "Failed to deserialize query string: missing field `to`".to_string(),
            status: StatusCode::BAD_REQUEST,
        });

        let q_string = "client_id=1".to_string();
        let req_body = axum::body::Body::empty();

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_missing_client_id() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: "Failed to deserialize query string: missing field `client_id`".to_string(),
            status: StatusCode::BAD_REQUEST,
        });

        let q_string = "cli=1&to=2".to_string();
        let req_body = axum::body::Body::empty();

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }
}
