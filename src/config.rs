use std::net::SocketAddr;

use config::{Environment, File};
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub redis_url: String,
    pub redis_conn_timeout_sec: u64,
    pub server_address: SocketAddr,
}

impl Config {
    pub fn new() -> Result<Self, config::ConfigError> {
        let cfg = config::Config::builder()
            .add_source(File::with_name("config/default.yml"))
            .add_source(Environment::with_prefix("TB"))
            .build()?;

        cfg.try_deserialize()
    }
}
