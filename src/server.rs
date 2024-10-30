use crate::config::Config;
use crate::handlers::{message_handler, sse_handler};
use crate::message_courier::MessageCourier;
use crate::storage::EventStorage;
use axum::extract::{MatchedPath, Request};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::{
    routing::{get, post},
    Router,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::future::ready;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
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
        .route("/health", get(healthcheck))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
                .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
        )
        .route_layer(middleware::from_fn(http_metrics_middleware))
        .with_state(app_state)
}

pub fn metrics_router() -> Router {
    let recorder = PrometheusBuilder::new().install_recorder().unwrap();
    Router::new().route("/metrics", get(move || ready(recorder.render())))
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
    pub config: Config,
    pub event_saver: Arc<S>,
    pub subscription_manager: Arc<C>,
}

async fn http_metrics_middleware(req: Request, next: Next) -> impl IntoResponse {
    let method = req.method().as_str().to_owned();
    let path = if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
        matched_path.as_str().to_owned()
    } else {
        "invalid".to_owned()
    };

    let start = Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed().as_secs_f64();

    let status = response.status().as_u16().to_string();
    let labels = [("method", method), ("path", path), ("status", status)];
    metrics::counter!("http_requests_total", &labels).increment(1);
    metrics::histogram!("http_requests_duration_seconds", &labels).record(duration);

    response
}

async fn healthcheck() -> axum::response::Result<String> {
    Ok("OK".into())
}
