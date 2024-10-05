use std::net::SocketAddr;

use config::{Environment, File};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub redis_url: String,
    pub redis_conn_timeout_sec: u64,
    pub server_address: SocketAddr,

    pub sse_heartbeat_interval_sec: u64,
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
    /// to reduce the redis resources consumption.
    pub inbox_inactive_ttl_sec: u16,

    pub max_client_ids_per_connection: usize,
}
impl Config {
    pub fn new() -> Result<Self, config::ConfigError> {
        let cfg = config::Config::builder()
            .add_source(File::with_name("config/default.yml"))
            .add_source(Environment::default().prefix("APP"))
            .build()?;

        cfg.try_deserialize()
    }
}
