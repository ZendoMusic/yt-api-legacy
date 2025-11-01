use serde::{Deserialize, Serialize};
use std::fs;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct Config {
    /// YouTube API key
    pub api_key: String,
    /// Main URL for the API
    pub mainurl: String,
    /// Default video quality
    pub default_quality: String,
    /// List of available video qualities
    pub available_qualities: Vec<String>,
    /// Port number for the server (default: 2823)
    #[serde(default = "default_port")]
    pub port: u16,
    /// Request timeout in seconds
    pub request_timeout: u64,
    /// Whether to use thumbnail proxy
    pub use_thumbnail_proxy: bool,
    /// Whether to use channel thumbnail proxy
    pub use_channel_thumbnail_proxy: bool,
    /// Whether to use video proxy
    pub use_video_proxy: bool,
    /// Video source URL
    pub video_source: String,
    /// Whether to fetch channel thumbnails
    pub fetch_channel_thumbnails: bool,
    /// Whether to use cookies
    pub use_cookies: bool,
    /// OAuth client ID
    pub oauth_client_id: String,
    /// OAuth client secret
    pub oauth_client_secret: String,
    /// Secret key for the application
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