use std::fs;
use std::io::{self, Write};
use std::path::Path;
use tokio::io::AsyncWriteExt;

pub async fn perform_startup_checks() {
    log::info!("Performing startup checks...");
    check_and_generate_config();
    log::info!("Startup checks completed.");
}

fn check_and_generate_config() {
    if !Path::new("config.yml").exists() {
        log::warn!("config.yml not found. Generating default config...");

        let default_config = r#"server:
  port: 2823
  main_url: ""
  secret_key: ""

api:
  request_timeout: 30
  keys:
    active: []
    disabled: []
  innertube:
    key: ""
  oauth:
    client_id: ""
    client_secret: ""
    redirect_uri: null

video:
  source: "innertube"
  use_cookies: false
  default_quality: "360"
  available_qualities:
    - "144"
    - "240"
    - "360"
    - "480"
    - "720"
    - "1080"
    - "1440"
    - "2160"
  default_count: 50

proxy:
  thumbnails:
    video: true
    channel: false
    fetch_channel_thumbnails: false
  video_proxy: true

cache:
  temp_folder_max_size_mb: 5120
  cleanup_threshold_mb: 100

instances:
  - "https://yt.legacyprojects.ru"
  - "https://yt.modyleprojects.ru"
  - "https://ytcloud.meetlook.ru"
"#;

        if let Err(e) = fs::write("config.yml", default_config) {
            log::error!("Failed to create default config.yml: {}", e);
            std::process::exit(1);
        }

        log::info!("Default config.yml created. Please update it with your actual values.");
    } else {
        log::info!("CHECK: config.yml found.");
    }
}
