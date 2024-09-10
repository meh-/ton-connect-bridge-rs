use crate::models::TonEvent;
use crate::server::AppState;
use crate::storage::EventStorage;
use axum::{extract::State, http::StatusCode, Json};
use base64::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{AppError, Query, ValidationError};

const MAX_TTL_SECS: u16 = 500;
const MIN_ALLOWED_TTL_SECS: u16 = 10;
const MAX_CLIENT_ID_LEN: usize = 64;

pub async fn message_handler<S, C>(
    Query(query): Query<SendMessageQueryParams>,
    State(state): State<AppState<S, C>>,
    body: String,
) -> Result<Json<SendMessageResponse>, AppError>
where
    S: EventStorage,
{
    query.validate()?;
    validate_message_body(&body)?;

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
        from: query.client_id,
        to: query.to,
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

impl SendMessageQueryParams {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.client_id.is_empty() {
            return Err(ValidationError(
                "Failed to deserialize query string: empty field `client_id`".into(),
            ));
        }
        if self.client_id.len() > MAX_CLIENT_ID_LEN {
            return Err(ValidationError(
                format!("Failed to deserialize query string: field `client_id` must not exceed {MAX_CLIENT_ID_LEN} characters"),
            ));
        }

        if self.to.is_empty() {
            return Err(ValidationError(
                "Failed to deserialize query string: empty field `to`".into(),
            ));
        }
        if self.to.len() > MAX_CLIENT_ID_LEN {
            return Err(ValidationError(
                format!("Failed to deserialize query string: field `to` must not exceed {MAX_CLIENT_ID_LEN} characters"),
            ));
        }

        if let Some(ttl) = self.ttl {
            if ttl > MAX_TTL_SECS {
                return Err(ValidationError(
                    format!("Failed to deserialize query string: `ttl` value must be less than {MAX_TTL_SECS}"),
                ));
            }
        }

        Ok(())
    }
}

fn validate_message_body(body: &String) -> Result<(), ValidationError> {
    if body.is_empty() {
        return Err(ValidationError("Missing request body".into()));
    }
    match BASE64_STANDARD.decode(&body) {
        Ok(_) => (),
        Err(_) => {
            return Err(ValidationError(
                "Request body must be a valid base64 message".into(),
            ))
        }
    }
    Ok(())
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

    #[tokio::test]
    async fn test_empty_client_id() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: "Failed to deserialize query string: empty field `client_id`".to_string(),
            status: StatusCode::BAD_REQUEST,
        });

        let q_string = "client_id=&to=2".to_string();
        let req_body = axum::body::Body::empty();

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_too_long_client_id() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: format!("Failed to deserialize query string: field `client_id` must not exceed {MAX_CLIENT_ID_LEN} characters"),
            status: StatusCode::BAD_REQUEST,
        });

        let long_id = "a".repeat(MAX_CLIENT_ID_LEN + 1);
        let q_string = format!("client_id={long_id}&to=2");
        let req_body = axum::body::Body::empty();

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
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
    async fn test_empty_to() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: "Failed to deserialize query string: empty field `to`".to_string(),
            status: StatusCode::BAD_REQUEST,
        });

        let q_string = "client_id=1&to=".to_string();
        let req_body = axum::body::Body::empty();

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_too_long_to() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: format!("Failed to deserialize query string: field `to` must not exceed {MAX_CLIENT_ID_LEN} characters"),
            status: StatusCode::BAD_REQUEST,
        });

        let long_id = "a".repeat(MAX_CLIENT_ID_LEN + 1);
        let q_string = format!("client_id=1&to={long_id}");
        let req_body = axum::body::Body::empty();

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_large_ttl() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: format!(
                "Failed to deserialize query string: `ttl` value must be less than {MAX_TTL_SECS}"
            ),
            status: StatusCode::BAD_REQUEST,
        });

        let ttl = MAX_TTL_SECS + 1;
        let q_string = format!("client_id=1&to=2&ttl={ttl}");
        let req_body = axum::body::Body::empty();

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_missing_message_body() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: "Missing request body".into(),
            status: StatusCode::BAD_REQUEST,
        });

        let q_string = "client_id=1&to=2".to_string();
        let req_body = axum::body::Body::empty();

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_not_base64_request_body() {
        let saver_mock = MockStorage::new();
        let expected_resp = json!(AppError {
            message: "Request body must be a valid base64 message".into(),
            status: StatusCode::BAD_REQUEST,
        });

        let q_string = "client_id=1&to=2".to_string();
        let req_body = axum::body::Body::from("hello world!");

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp_body, expected_resp)
    }

    #[tokio::test]
    async fn test_valid_request_with_ttl() {
        let mut saver_mock = MockStorage::new();
        saver_mock
            .expect_add()
            .withf(|event: &TonEvent| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                event.id == ""
                    && event.from == "1"
                    && event.to == "2"
                    && event.message == BASE64_STANDARD.encode("hello world!")
                    // the difference between the deadline and current timestamp
                    // should be close to the specified in the request ttl
                    && (98..=100).contains(&(event.deadline - now))
            })
            .returning(|_| Box::pin(async { Ok(()) }));

        let expected_resp = json!(SendMessageResponse {
            code: StatusCode::OK.into(),
            message: "OK".to_owned(),
        });

        let q_string = "client_id=1&to=2&ttl=100".to_string();
        let req_body = axum::body::Body::from(BASE64_STANDARD.encode("hello world!"));

        let (status, resp_body) = exec(saver_mock, q_string, req_body).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp_body, expected_resp)
    }
}
