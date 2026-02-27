use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use tokio::io::AsyncWriteExt;

pub async fn perform_startup_checks() {
    log::info!("Performing startup checks...");
    check_and_generate_config();
    check_and_download_yt_dlp().await;
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
  source: "direct"
  use_cookies: true
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

async fn check_and_download_yt_dlp() {
    let yt_dlp_exists = Command::new("yt-dlp").arg("--version").output().is_ok()
        || Path::new("assets/yt-dlp").exists()
        || Path::new("assets/yt-dlp.exe").exists();

    if yt_dlp_exists {
        log::info!("CHECK: yt-dlp found.");
        return;
    }

    log::error!("CHECK: yt-dlp not found!");
    log::info!("Would you like to download the latest version of yt-dlp from GitHub? (y/n): ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read input");

    if input.trim().to_lowercase() == "y"
        || input.trim().to_lowercase() == "yes"
        || input.trim().eq_ignore_ascii_case("д")
        || input.trim().eq_ignore_ascii_case("да")
    {
        log::info!("Downloading latest yt-dlp...");

        match download_yt_dlp().await {
            Ok(_) => {
                log::info!("yt-dlp downloaded successfully!");
                log::info!("Please, reopen the server for changes to take effect.");
                std::process::exit(0);
            }
            Err(e) => {
                log::error!("Failed to download yt-dlp: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        log::error!("yt-dlp is required to run this server.");
        std::process::exit(1);
    }
}

async fn download_yt_dlp() -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new("assets").exists() {
        fs::create_dir("assets")?;
    }

    let client = reqwest::Client::new();

    let (url, binary_name) = if cfg!(target_os = "windows") {
        (
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
            "yt-dlp.exe",
        )
    } else {
        (
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux",
            "yt-dlp",
        )
    };

    let response = client.get(url).send().await?;
    let content = response.bytes().await?;

    let file_path = format!("assets/{}", binary_name);

    let mut file = tokio::fs::File::create(&file_path).await?;
    file.write_all(&content).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&file_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&file_path, perms)?;
    }

    Ok(())
}
