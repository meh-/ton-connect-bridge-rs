use crate::handlers::{message_handler, sse_handler};
use crate::message_courier::MessageCourier;
use crate::storage::RedisEventStorage;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::trace::{self, TraceLayer};
use tracing::Level;

pub fn router(app_state: AppState) -> Router {
    Router::new()
        .route("/events", get(sse_handler))
        .route("/message", post(message_handler))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
                .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(app_state)
}

pub async fn start(router: Router, port: u64) {
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .unwrap();

    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, router)
        .await
        .expect("failed to start server");
}

#[derive(Clone)]
pub struct AppState {
    pub event_saver: Arc<RedisEventStorage>,
    pub subscription_manager: Arc<MessageCourier>,
}
