use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub api_key: String,
    pub mainurl: String,
    pub default_quality: String,
    pub available_qualities: Vec<String>,
    #[serde(default = "default_port")]
    pub port: u16,
    pub request_timeout: u64,
    pub use_thumbnail_proxy: bool,
    pub use_channel_thumbnail_proxy: bool,
    pub use_video_proxy: bool,
    pub video_source: String,
    pub fetch_channel_thumbnails: bool,
    pub use_cookies: bool,
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    pub secretkey: String,
}

fn default_port() -> u16 {
    2823
}

impl Config {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&contents)?;
        Ok(config)
    }
}