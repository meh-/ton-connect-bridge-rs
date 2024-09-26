use crate::handlers::{message_handler, sse_handler};
use crate::message_courier::MessageCourier;
use crate::storage::EventStorage;
use axum::{
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::trace::{self, TraceLayer};
use tracing::Level;

pub fn router<S, C>(app_state: AppState<S, C>) -> Router
where
    S: EventStorage + Clone + 'static,
    C: MessageCourier + Clone + 'static,
{
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

pub async fn start(router: Router, address: SocketAddr) {
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();

    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, router)
        .await
        .expect("failed to start server");
}

#[derive(Clone)]
pub struct AppState<S, C> {
    pub event_saver: Arc<S>,
    pub subscription_manager: Arc<C>,
}
