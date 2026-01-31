use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_main_url")]
    pub main_url: String,
    #[serde(rename = "secret_key")]
    pub secretkey: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ApiKeysConfig {
    #[serde(default)]
    pub active: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
}

impl Default for ApiKeysConfig {
    fn default() -> Self {
        Self {
            active: Vec::new(),
            disabled: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct InnertubeConfig {
    #[serde(default)]
    pub key: Option<String>,
}

impl Default for InnertubeConfig {
    fn default() -> Self {
        Self { key: None }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct OAuthConfig {
    #[serde(rename = "client_id")]
    pub client_id: String,
    #[serde(rename = "client_secret")]
    pub client_secret: String,
    #[serde(rename = "redirect_uri")]
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ApiConfig {
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,
    #[serde(default)]
    pub keys: ApiKeysConfig,
    #[serde(default)]
    pub innertube: InnertubeConfig,
    #[serde(default)]
    pub oauth: OAuthConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct VideoConfig {
    #[serde(rename = "source")]
    pub source: String,
    #[serde(rename = "use_cookies")]
    pub use_cookies: bool,
    #[serde(rename = "default_quality")]
    pub default_quality: String,
    #[serde(rename = "available_qualities")]
    pub available_qualities: Vec<String>,
    #[serde(default = "default_count")]
    pub default_count: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ProxyThumbnailsConfig {
    pub video: bool,
    pub channel: bool,
    #[serde(rename = "fetch_channel_thumbnails")]
    pub fetch_channel_thumbnails: bool,
}

impl Default for ProxyThumbnailsConfig {
    fn default() -> Self {
        Self {
            video: false,
            channel: false,
            fetch_channel_thumbnails: false,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ProxyConfig {
    pub thumbnails: ProxyThumbnailsConfig,
    #[serde(rename = "video_proxy")]
    pub video_proxy: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            thumbnails: ProxyThumbnailsConfig::default(),
            video_proxy: false,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct CacheConfig {
    #[serde(rename = "temp_folder_max_size_mb")]
    #[serde(default = "temp_folder_max_size_mb")]
    pub temp_folder_max_size_mb: u32,
    #[serde(rename = "cleanup_threshold_mb")]
    #[serde(default = "cleanup_threshold_mb")]
    pub cleanup_threshold_mb: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
#[serde(transparent)]
pub struct InstantInstance(pub String);

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct Config {
    pub server: ServerConfig,
    pub api: ApiConfig,
    pub video: VideoConfig,
    pub proxy: ProxyConfig,
    pub cache: CacheConfig,
    #[serde(default)]
    #[serde(rename = "instances")]
    pub instants: Vec<InstantInstance>,
}

static API_KEY_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn default_port() -> u16 {
    2823
}

fn default_main_url() -> String {
    String::new()
}

fn default_request_timeout() -> u64 {
    30
}

fn default_count() -> u32 {
    50
}

fn temp_folder_max_size_mb() -> u32 {
    5120
}

fn cleanup_threshold_mb() -> u32 {
    100
}

fn normalize_url(input: &str) -> String {
    input.trim().trim_end_matches('/').to_lowercase()
}

fn parse_quality_value(value: &str) -> Option<u32> {
    let digits = value
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    digits.parse::<u32>().ok()
}

fn compare_quality(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_quality_value(a), parse_quality_value(b)) {
        (Some(a_val), Some(b_val)) => a_val.cmp(&b_val),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

impl Config {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    pub fn tidy(&mut self) {
        let mut clean_keys = self
            .api
            .keys
            .active
            .iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect::<Vec<_>>();
        clean_keys.sort();
        clean_keys.dedup();
        self.api.keys.active = clean_keys;

        let mut clean_failed = self
            .api
            .keys
            .disabled
            .iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect::<Vec<_>>();
        clean_failed.sort();
        clean_failed.dedup();
        self.api.keys.disabled = clean_failed;

        self.video
            .available_qualities
            .sort_by(|a, b| compare_quality(a, b));
        self.video.available_qualities.dedup();

        self.instants
            .sort_by(|a, b| normalize_url(&a.0).cmp(&normalize_url(&b.0)));
        let mut seen = HashSet::new();
        self.instants
            .retain(|inst| seen.insert(normalize_url(&inst.0)));
    }

    pub fn persist(&mut self, path: &str) -> Result<(), String> {
        self.tidy();
        serde_yaml::to_string(&self)
            .map_err(|e| format!("Failed to serialize config: {}", e))
            .and_then(|yaml| {
                fs::write(path, yaml).map_err(|e| format!("Failed to write config: {}", e))
            })
    }

    pub fn get_api_key_rotated(&self) -> &str {
        let bad: HashSet<&str> = self.api.keys.disabled.iter().map(|s| s.as_str()).collect();
        let good_keys: Vec<&str> = self
            .api
            .keys
            .active
            .iter()
            .map(|s| s.as_str())
            .filter(|k| !k.is_empty() && !bad.contains(k))
            .collect();
        
        // Handle case when no API keys are configured
        if good_keys.is_empty() {
            return "AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8"; // Default YouTube API key
        }
        
        let index = API_KEY_COUNTER.fetch_add(1, Ordering::Relaxed) % good_keys.len();
        good_keys[index]
    }

    pub fn get_innertube_key(&self) -> Option<&str> {
        self.api
            .innertube
            .key
            .as_deref()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
    }
}
