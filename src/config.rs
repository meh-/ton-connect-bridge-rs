use std::net::SocketAddr;

use config::{Environment, File};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    /// The log level for the application.
    /// Available values are: debug, info, error.
    pub log_level: String,
    /// The URL for connecting to the Redis server.
    pub redis_url: String,
    /// The timeout duration for Redis connections, in seconds.
    pub redis_conn_timeout_sec: u64,
    /// The address for the main bridge server: host + port. Example: `http://localhost:3000`.
    pub server_address: SocketAddr,
    /// The address for the metrics server: host + port. Example: `http://localhost:3001`
    pub metrics_server_address: SocketAddr,

    /// The interval for sending heartbeat messages over SSE, in seconds.
    pub sse_heartbeat_interval_sec: u64,
    /// For how long it should be allowed to keep an open sse connection if there aren't messages.
    pub sse_client_without_messages_ttl_sec: u64,
    /// Max ttl for an individual message.
    pub inbox_max_message_ttl_sec: u16,
    /// If the ttl parameter in an incoming request is less than this min ttl,
    /// then the max ttl will be applied instead.
    pub inbox_min_message_ttl_sec: u16,
    /// How many messages can be stored per client. This parameter is used to better manage
    /// storage's resources consumption. If there are more messages for a client than the specified number,
    /// older messages will be dropped first.
    pub inbox_max_messages_per_client: usize,
    /// If there are no new messages within the specified ttl
    /// for a client, then the whole client's inbox (messages) will be dropped
    /// to reduce the storage's resources consumption.
    pub inbox_inactive_ttl_sec: u16,

    /// How many client ids can be passed to the sse endpoint in a single request.
    pub max_client_ids_per_connection: usize,

    /// Size of the global messages stream should be enough to handle
    /// short (less than a minute) consumers outages, and do not loose any messages
    /// even if producers are actively pushing new messages while consumers are down.
    /// Obviously the bigger size the more resources Redis requires
    pub global_stream_max_size: usize,
}
impl Config {
    pub fn new() -> Result<Self, config::ConfigError> {
        let cfg = config::Config::builder()
            .add_source(File::with_name("config/default.yml"))
            .add_source(Environment::default().prefix("APP"))
            .build()?;

        cfg.try_deserialize()
    }

    pub fn log_level(&self) -> tracing::Level {
        match self.log_level.to_lowercase().as_str() {
            "debug" => tracing::Level::DEBUG,
            "info" => tracing::Level::INFO,
            "error" => tracing::Level::ERROR,
            _ => tracing::Level::INFO,
        }
    }
}
