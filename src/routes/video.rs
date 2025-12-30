use actix_web::{web, HttpResponse, Responder, HttpRequest};
use actix_web::http::header::{HeaderValue, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION};
use reqwest::Client;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use lru::LruCache;
use tokio::sync::Mutex;
use tokio::task;
use lazy_static::lazy_static;
use serde::Serialize;
use utoipa::ToSchema;
use chrono::DateTime;
use urlencoding;

lazy_static! {
    static ref THUMBNAIL_CACHE: Arc<Mutex<LruCache<String, (Vec<u8>, String, u64)>>> = 
        Arc::new(Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1000).unwrap())));
}

const CACHE_DURATION: u64 = 3600;

fn yt_dlp_binary() -> String {
    if cfg!(target_os = "windows") {
        if Path::new("assets/yt-dlp.exe").exists() {
            return "assets/yt-dlp.exe".to_string();
        }
    } else if Path::new("assets/yt-dlp").exists() {
        return "assets/yt-dlp".to_string();
    }
    "yt-dlp".to_string()
}

async fn resolve_direct_stream_url(
    video_id: &str,
    quality: Option<&str>,
    audio_only: bool,
    config: &crate::config::Config,
) -> Option<String> {
    let video_id = video_id.to_string();
    let quality = quality
        .map(|q| q.to_string())
        .unwrap_or_else(|| config.video.default_quality.clone());
    let use_cookies = config.video.use_cookies;
    let yt_dlp = yt_dlp_binary();
    
    task::spawn_blocking(move || {
        let url = format!("https://www.youtube.com/watch?v={}", video_id);
        let format_selector = if audio_only {
            "bestaudio/best".to_string()
        } else {
            format!("best[height<={}][ext=mp4]/best[ext=mp4]/best", quality)
        };
        
        let mut cmd = Command::new(yt_dlp);
        cmd.arg("-f")
            .arg(format_selector)
            .arg("--get-url")
            .arg(&url);
        
        if use_cookies {
            let cookie_paths = ["assets/cookies.txt", "cookies.txt"];
            for path in &cookie_paths {
                if Path::new(path).exists() {
                    cmd.arg("--cookies").arg(path);
                    break;
                }
            }
        }
        
        match cmd.output() {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.lines().find(|l| !l.trim().is_empty()).map(|s| s.to_string())
            }
            _ => None,
        }
    })
    .await
    .ok()
    .flatten()
}

async fn proxy_stream_response(
    target_url: &str,
    req: &HttpRequest,
    default_content_type: &str,
) -> HttpResponse {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()
        .unwrap();
    
    let mut request_builder = client.get(target_url);
    if let Some(range_header) = req.headers().get("Range") {
        request_builder = request_builder.header("Range", range_header.clone());
    }
    
    match request_builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            let content_type = headers
                .get(CONTENT_TYPE)
                .and_then(|ct| ct.to_str().ok())
                .unwrap_or(default_content_type)
                .to_string();
            
            let stream = resp.bytes_stream().map(|item| {
                item.map_err(|e| actix_web::error::ErrorBadGateway(e))
            });
            
            let mut builder = HttpResponse::build(status);
            for (key, value) in headers.iter() {
                // Skip hop-by-hop headers
                if key == "connection" || key == "transfer-encoding" {
                    continue;
                }
                builder.insert_header((key.clone(), value.clone()));
            }
            builder.insert_header((CONTENT_TYPE, HeaderValue::from_str(&content_type).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))));
            builder.streaming(stream)
        }
        Err(e) => {
            crate::log::info!("Proxy request failed: {}", e);
            HttpResponse::BadGateway().json(serde_json::json!({
                "error": "Failed to proxy request"
            }))
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct VideoInfoResponse {
    pub title: String,
    pub author: String,
    #[serde(rename = "subscriberCount")]
    pub subscriber_count: String,
    pub description: String,
    pub video_id: String,
    pub embed_url: String,
    pub duration: String,
    pub published_at: String,
    pub likes: Option<String>,
    pub views: Option<String>,
    pub comment_count: Option<String>,
    pub comments: Vec<Comment>,
    pub channel_thumbnail: String,
    pub thumbnail: String,
    pub video_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Comment {
    pub author: String,
    pub text: String,
    pub published_at: String,
    pub author_thumbnail: String,
}

#[derive(Serialize, ToSchema)]
pub struct RelatedVideo {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub views: String,
    pub published_at: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
    pub url: String,
    pub source: String,
}

#[derive(Serialize, ToSchema)]
pub struct DirectUrlResponse {
    pub video_url: String,
}

#[utoipa::path(
    get,
    path = "/thumbnail/{video_id}",
    params(
        ("video_id" = String, Path, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Thumbnail quality (default, medium, high, standard, maxres)")
    ),
    responses(
        (status = 200, description = "Thumbnail image", content_type = "image/jpeg"),
        (status = 404, description = "Thumbnail not found")
    )
)]
pub async fn thumbnail_proxy(
    path: web::Path<String>,
    req: HttpRequest,
) -> impl Responder {
    let video_id = path.into_inner();
    
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let quality = query_params.get("quality").map(|s| s.as_str()).unwrap_or("medium");
    
    let quality_map = [
        ("default", "default.jpg"),
        ("medium", "mqdefault.jpg"),
        ("high", "hqdefault.jpg"),
        ("standard", "sddefault.jpg"),
        ("maxres", "maxresdefault.jpg"),
    ];
    
    let thumbnail_type = quality_map.iter()
        .find(|(q, _)| *q == quality)
        .map(|(_, t)| *t)
        .unwrap_or("mqdefault.jpg");
    
    let cache_key = format!("{}_{}", video_id, thumbnail_type);
    
    {
        let mut cache = THUMBNAIL_CACHE.lock().await;
        if let Some((data, content_type, timestamp)) = cache.get(&cache_key) {
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            if current_time - timestamp < CACHE_DURATION {
                return HttpResponse::Ok()
                    .content_type(content_type.as_str())
                    .body(data.clone());
            }
        }
    }
    
    let url = format!("https://i.ytimg.com/vi/{}/{}", video_id, thumbnail_type);
    
    let client = Client::new();
    
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let headers = resp.headers().clone();
            if status == 404 && thumbnail_type != "mqdefault.jpg" {
                let fallback_url = format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", video_id);
                match client.get(&fallback_url).send().await {
                    Ok(fallback_resp) => {
                        let fallback_headers = fallback_resp.headers().clone();
                        let content_type = fallback_headers.get("content-type")
                            .and_then(|ct| ct.to_str().ok())
                            .unwrap_or("image/jpeg")
                            .to_string();
                        
                        match fallback_resp.bytes().await {
                            Ok(bytes) => {
                                let current_time = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs();
                                
                                let mut cache = THUMBNAIL_CACHE.lock().await;
                                cache.put(cache_key, (bytes.to_vec(), content_type.clone(), current_time));
                                
                                HttpResponse::Ok()
                                    .content_type(content_type.as_str())
                                    .body(bytes)
                            },
                            Err(_) => HttpResponse::NotFound().finish(),
                        }
                    }
                    Err(_) => HttpResponse::NotFound().finish(),
                }
            } else {
                let content_type = headers.get("content-type")
                    .and_then(|ct| ct.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();
                
                match resp.bytes().await {
                    Ok(bytes) => {
                        let current_time = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs();
                        
                        let mut cache = THUMBNAIL_CACHE.lock().await;
                        cache.put(cache_key, (bytes.to_vec(), content_type.clone(), current_time));
                        
                        HttpResponse::Ok()
                            .content_type(content_type.as_str())
                            .body(bytes)
                    },
                    Err(_) => HttpResponse::NotFound().finish(),
                }
            }
        }
        Err(_) => HttpResponse::NotFound().finish(),
    }
}

#[utoipa::path(
    get,
    path = "/channel_icon/{path_video_id}",
    params(
        ("path_video_id" = String, Path, description = "Channel ID, video ID, or direct image URL")
    ),
    responses(
        (status = 200, description = "Channel icon image", content_type = "image/jpeg"),
        (status = 404, description = "Channel icon not found")
    )
)]
pub async fn channel_icon(
    path: web::Path<String>,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let path_video_id = path.into_inner();
    let config = &data.config;
    
    if path_video_id.starts_with("http") {
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
            .build()
            .unwrap();
        
        match client.get(&path_video_id).send().await {
            Ok(image_resp) => {
                let headers = image_resp.headers().clone();
                let content_type = headers.get("content-type")
                    .and_then(|ct| ct.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();
                
                match image_resp.bytes().await {
                    Ok(bytes) => HttpResponse::Ok()
                        .content_type(content_type.as_str())
                        .insert_header(("Cache-Control", "public, max-age=3600"))
                        .body(bytes),
                    Err(_) => HttpResponse::NotFound().finish(),
                }
            }
            Err(_) => HttpResponse::NotFound().finish(),
        }
    } else {
        let apikey = config.get_api_key_rotated();
        let quality = "default";
        
        let channel_id = if path_video_id.starts_with("UC") {
            path_video_id.clone()
        } else if path_video_id.starts_with('@') {
            let username = &path_video_id[1..];
            let search_url = format!(
                "https://www.googleapis.com/youtube/v3/channels?forUsername={}&key={}&part=id",
                username, apikey
            );
            
            let client = Client::new();
            match client.get(&search_url).send().await {
                Ok(search_resp) => {
                    match search_resp.json::<serde_json::Value>().await {
                        Ok(search_data) => {
                            if let Some(items) = search_data.get("items").and_then(|i| i.as_array()) {
                                if !items.is_empty() {
                                    if let Some(channel_id) = items[0].get("id").and_then(|id| id.as_str()) {
                                        channel_id.to_string()
                                    } else {
                                        return HttpResponse::NotFound().finish();
                                    }
                                } else {
                                    let search_url = format!(
                                        "https://www.googleapis.com/youtube/v3/search?part=snippet&q={}&type=channel&key={}",
                                        username, apikey
                                    );
                                    
                                    match client.get(&search_url).send().await {
                                        Ok(search_resp) => {
                                            match search_resp.json::<serde_json::Value>().await {
                                                Ok(search_data) => {
                                                    if let Some(items) = search_data.get("items").and_then(|i| i.as_array()) {
                                                        if !items.is_empty() {
                                                            if let Some(channel_id) = items[0]
                                                                .get("snippet")
                                                                .and_then(|s| s.get("channelId"))
                                                                .and_then(|id| id.as_str()) {
                                                                channel_id.to_string()
                                                            } else {
                                                                return HttpResponse::NotFound().finish();
                                                            }
                                                        } else {
                                                            return HttpResponse::NotFound().finish();
                                                        }
                                                    } else {
                                                        return HttpResponse::NotFound().finish();
                                                    }
                                                }
                                                Err(_) => return HttpResponse::NotFound().finish(),
                                            }
                                        }
                                        Err(_) => return HttpResponse::NotFound().finish(),
                                    }
                                }
                            } else {
                                return HttpResponse::NotFound().finish();
                            }
                        }
                        Err(_) => return HttpResponse::NotFound().finish(),
                    }
                }
                Err(_) => return HttpResponse::NotFound().finish(),
            }
        } else {
            let video_url = format!(
                "https://www.googleapis.com/youtube/v3/videos?id={}&key={}&part=snippet",
                path_video_id, apikey
            );
            
            let client = Client::new();
            match client.get(&video_url).send().await {
                Ok(video_resp) => {
                    match video_resp.json::<serde_json::Value>().await {
                        Ok(video_data) => {
                            if let Some(items) = video_data.get("items").and_then(|i| i.as_array()) {
                                if !items.is_empty() {
                                    if let Some(channel_id) = items[0]
                                        .get("snippet")
                                        .and_then(|s| s.get("channelId"))
                                        .and_then(|id| id.as_str()) {
                                        channel_id.to_string()
                                    } else {
                                        return HttpResponse::NotFound().finish();
                                    }
                                } else {
                                    return HttpResponse::NotFound().finish();
                                }
                            } else {
                                return HttpResponse::NotFound().finish();
                            }
                        }
                        Err(_) => return HttpResponse::NotFound().finish(),
                    }
                }
                Err(_) => return HttpResponse::NotFound().finish(),
            }
        };
        
        let channel_url = format!(
            "https://www.googleapis.com/youtube/v3/channels?id={}&key={}&part=snippet",
            channel_id, apikey
        );
        
        let client = Client::new();
        match client.get(&channel_url).send().await {
            Ok(channel_resp) => {
                match channel_resp.json::<serde_json::Value>().await {
                    Ok(channel_data) => {
                        if let Some(items) = channel_data.get("items").and_then(|i| i.as_array()) {
                            if !items.is_empty() {
                                if let Some(thumbnails) = items[0]
                                    .get("snippet")
                                    .and_then(|s| s.get("thumbnails")) {
                                    
                                    let quality_map = [
                                        ("default", "default"),
                                        ("medium", "medium"),
                                        ("high", "high"),
                                    ];
                                    
                                    let thumbnail_quality = quality_map.iter()
                                        .find(|(q, _)| *q == quality)
                                        .map(|(_, t)| *t)
                                        .unwrap_or("default");
                                    
                                    let mut selected_quality = thumbnail_quality;
                                    if thumbnails.get(thumbnail_quality).is_none() {
                                        let fallback_order = [thumbnail_quality, "default", "medium", "high"];
                                        for q in &fallback_order {
                                            if thumbnails.get(q).is_some() {
                                                selected_quality = q;
                                                break;
                                            }
                                        }
                                    }
                                    
                                    if let Some(thumbnail_url) = thumbnails
                                        .get(selected_quality)
                                        .and_then(|t| t.get("url"))
                                        .and_then(|url| url.as_str()) {
                                        
                                        let mut final_url = thumbnail_url.to_string();
                                        if final_url.contains("yt3.ggpht.com") {
                                            final_url = final_url.replace("yt3.ggpht.com", "yt3.googleusercontent.com");
                                        }
                                        
                                        let client = Client::builder()
                                            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
                                            .build()
                                            .unwrap();
                                        
                                        match client.get(&final_url).send().await {
                                            Ok(image_resp) => {
                                                let headers = image_resp.headers().clone();
                                                let content_type = headers.get("content-type")
                                                    .and_then(|ct| ct.to_str().ok())
                                                    .unwrap_or("image/jpeg")
                                                    .to_string();
                                                
                                                match image_resp.bytes().await {
                                                    Ok(bytes) => HttpResponse::Ok()
                                                        .content_type(content_type.as_str())
                                                        .insert_header(("Cache-Control", "public, max-age=3600"))
                                                        .body(bytes),
                                                    Err(_) => HttpResponse::NotFound().finish(),
                                                }
                                            }
                                            Err(_) => HttpResponse::NotFound().finish(),
                                        }
                                    } else {
                                        HttpResponse::NotFound().finish()
                                    }
                                } else {
                                    HttpResponse::NotFound().finish()
                                }
                            } else {
                                HttpResponse::NotFound().finish()
                            }
                        } else {
                            HttpResponse::NotFound().finish()
                        }
                    }
                    Err(_) => HttpResponse::NotFound().finish(),
                }
            }
            Err(_) => HttpResponse::NotFound().finish(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/get-ytvideo-info.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Video quality"),
        ("proxy" = Option<String>, Query, description = "Use video proxy (true/false)")
    ),
    responses(
        (status = 200, description = "Video information", body = VideoInfoResponse),
        (status = 400, description = "Missing video ID"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_ytvideo_info(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    

    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID видео не был передан."
            }));
        }
    };
    
    let _quality = query_params.get("quality").map(|s| s.as_str()).unwrap_or(&config.video.default_quality);
    let proxy_param = query_params.get("proxy").map(|s| s.to_lowercase()).unwrap_or("true".to_string());
    let _use_video_proxy = proxy_param != "false";
    
    let apikey = config.get_api_key_rotated();
    let client = Client::new();
    

    let video_url = format!(
        "https://www.googleapis.com/youtube/v3/videos?id={}&key={}&part=snippet,contentDetails,statistics",
        video_id, apikey
    );
    
    match client.get(&video_url).send().await {
        Ok(video_resp) => {
            match video_resp.json::<serde_json::Value>().await {
                Ok(video_data) => {

                    let video_items = match video_data.get("items").and_then(|i| i.as_array()) {
                        Some(items) => items,
                        None => {
                            return HttpResponse::Ok().json(serde_json::json!({
                                "error": "Видео не найдено."
                            }));
                        }
                    };
                    
                    if video_items.is_empty() {
                        return HttpResponse::Ok().json(serde_json::json!({
                            "error": "Видео не найдено."
                            }));
                    }
                    
                    let video_item = &video_items[0];
                    let video_info = match video_item.get("snippet") {
                        Some(info) => info,
                        None => {
                            crate::log::info!("Error: Video snippet not found");
                            return HttpResponse::InternalServerError().json(serde_json::json!({
                                "error": "Internal server error"
                            }));
                        }
                    };
                    
                    let content_details = match video_item.get("contentDetails") {
                        Some(details) => details,
                        None => {
                            crate::log::info!("Error: Content details not found");
                            return HttpResponse::InternalServerError().json(serde_json::json!({
                                "error": "Internal server error"
                            }));
                        }
                    };
                    
                    let statistics = video_item.get("statistics").unwrap_or(&serde_json::Value::Null);
                    let channel_id = match video_info.get("channelId").and_then(|id| id.as_str()) {
                        Some(id) => id,
                        None => {
                            crate::log::info!("Error: Channel ID not found");
                            return HttpResponse::InternalServerError().json(serde_json::json!({
                                "error": "Internal server error"
                            }));
                        }
                    };
                    

                    let mut subscriber_count = "0".to_string();
                    let channel_url = format!(
                        "https://www.googleapis.com/youtube/v3/channels?id={}&key={}&part=snippet,statistics",
                        channel_id, apikey
                    );
                    
                    match client.get(&channel_url).send().await {
                        Ok(channel_resp) => {
                            match channel_resp.json::<serde_json::Value>().await {
                                Ok(channel_data) => {
                                    if let Some(channel_items) = channel_data.get("items").and_then(|i| i.as_array()) {
                                        if !channel_items.is_empty() {
                                            if let Some(stats) = channel_items[0].get("statistics") {
                        subscriber_count = stats.get("subscriberCount")
                            .and_then(|c| c.as_str())
                            .unwrap_or("0")
                            .to_string();
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    crate::log::info!("Error parsing channel data: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            crate::log::info!("Error fetching channel data: {}", e);
                        }
                    }
                    

                    let final_video_url = if config.video.video_source == "direct" {
                        format!("{}direct_url?video_id={}", config.server.mainurl, video_id)
                    } else {
                        "".to_string()
                    };
                    
                    let final_video_url_with_proxy = if config.proxy.use_video_proxy && !final_video_url.is_empty() {
                        format!("{}video.proxy?url={}", config.server.mainurl, urlencoding::encode(&final_video_url))
                    } else {
                        final_video_url.clone()
                    };
                    

                    let mut comments: Vec<Comment> = Vec::new();
                    let comments_url = format!(
                        "https://www.googleapis.com/youtube/v3/commentThreads?key={}&textFormat=plainText&part=snippet&videoId={}&maxResults=25",
                        apikey, video_id
                    );
                    
                    match client.get(&comments_url).send().await {
                        Ok(comments_resp) => {
                            match comments_resp.json::<serde_json::Value>().await {
                                Ok(comments_data) => {
                                    if let Some(comment_items) = comments_data.get("items").and_then(|i| i.as_array()) {
                                        for item in comment_items {
                                            if let Some(comment_snippet) = item
                                                .get("snippet")
                                                .and_then(|s| s.get("topLevelComment"))
                                                .and_then(|c| c.get("snippet")) {
                                                
                                                let author = comment_snippet.get("authorDisplayName")
                                                    .and_then(|a| a.as_str())
                                                    .unwrap_or("Unknown")
                                                    .to_string();
                                                
                                                let text = comment_snippet.get("textDisplay")
                                                    .and_then(|t| t.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                
                                                let published_at = comment_snippet.get("publishedAt")
                                                    .and_then(|p| p.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                
                        let author_thumbnail = if config.proxy.use_channel_thumbnail_proxy {
                            format!("{}channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id)
                        } else {
                            "".to_string()
                        };
                                                
                                                comments.push(Comment {
                                                    author,
                                                    text,
                                                    published_at,
                                                    author_thumbnail,
                                                });
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    crate::log::info!("Error parsing comments data: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            crate::log::info!("Error fetching comments: {}", e);
                        }
                    }
                    

                    let published_at = match video_info.get("publishedAt").and_then(|p| p.as_str()) {
                        Some(date_str) => {
                            if let Ok(datetime) = DateTime::parse_from_rfc3339(date_str) {
                                datetime.format("%d.%m.%Y, %H:%M:%S").to_string()
                            } else {
                                date_str.to_string()
                            }
                        }
                        None => "".to_string(),
                    };
                    

                    let response = VideoInfoResponse {
                        title: video_info.get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string(),
                        author: video_info.get("channelTitle")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string(),
                        subscriber_count,
                        description: video_info.get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string(),
                        video_id: video_id.clone(),
                        embed_url: format!("https://www.youtube.com/embed/{}", video_id),
                        duration: content_details.get("duration")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string(),
                        published_at,
                        likes: statistics.get("likeCount")
                            .and_then(|l| l.as_str())
                            .map(|s| s.to_string()),
                        views: statistics.get("viewCount")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        comment_count: statistics.get("commentCount")
                            .and_then(|c| c.as_str())
                            .map(|s| s.to_string()),
                        comments,
                        channel_thumbnail: format!("{}channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id),
                        thumbnail: format!("{}thumbnail/{}", config.server.mainurl.trim_end_matches('/'), video_id),
                        video_url: final_video_url_with_proxy,
                    };
                    
                    HttpResponse::Ok().json(response)
                }
                Err(e) => {
                    crate::log::info!("Error parsing video data: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Internal server error"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error fetching video data: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Internal server error"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/get_related_videos.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("count" = Option<i32>, Query, description = "Number of related videos to return (default: 50)"),
        ("offset" = Option<i32>, Query, description = "Offset for pagination (default: 0)"),
        ("limit" = Option<i32>, Query, description = "Limit for pagination (default: 50)"),
        ("order" = Option<String>, Query, description = "Order of results (relevance, date, rating, viewCount, title) (default: relevance)"),
        ("token" = Option<String>, Query, description = "Refresh token for InnerTube recommendations")
    ),
    responses(
        (status = 200, description = "List of related videos", body = [RelatedVideo]),
        (status = 400, description = "Missing video ID"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_related_videos(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID видео не был передан."
            }));
        }
    };
    
    // Handle count parameter (backward compatibility)
    let count_param: i32 = query_params.get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);
    
    // Handle limit parameter (takes precedence over count)
    let limit: i32 = query_params.get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(count_param);
    
    // Handle offset parameter
    let offset: i32 = query_params.get("offset")
        .and_then(|o| o.parse().ok())
        .unwrap_or(0);
    
    // Handle order parameter
    let order = query_params.get("order")
        .map(|o| o.as_str())
        .unwrap_or("relevance");
    
    // Validate order parameter
    let valid_orders = ["relevance", "date", "rating", "viewCount", "title"];
    let search_order = if valid_orders.contains(&order) {
        order
    } else {
        "relevance"
    };
    
    // Ensure limit is within reasonable bounds
    let max_results = limit.min(50).max(1);
    
    let apikey = config.get_api_key_rotated();
    let client = Client::new();
    
    // First, get the video information to create a search query
    let video_url = format!(
        "https://www.googleapis.com/youtube/v3/videos?part=snippet&id={}&key={}",
        video_id, apikey
    );
    
    let mut related_videos: Vec<RelatedVideo> = Vec::new();
    
    match client.get(&video_url).send().await {
        Ok(video_resp) => {
            match video_resp.json::<serde_json::Value>().await {
                Ok(video_data) => {
                    if let Some(video_items) = video_data.get("items").and_then(|i| i.as_array()) {
                        if !video_items.is_empty() {
                            if let Some(video_info) = video_items[0].get("snippet") {
                                // Create search query based on the video title
                                let title = video_info.get("title")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("");
                                
                                // Use the first word of the title for search
                                let search_query = title.split_whitespace().next().unwrap_or(title);
                                
                                // Search for related videos with ordering
                                let search_url = format!(
                                    "https://www.googleapis.com/youtube/v3/search?part=snippet&q={}&type=video&maxResults={}&order={}&key={}",
                                    urlencoding::encode(search_query),
                                    max_results,
                                    search_order,
                                    apikey
                                );
                                
                                match client.get(&search_url).send().await {
                                    Ok(search_resp) => {
                                        match search_resp.json::<serde_json::Value>().await {
                                            Ok(search_data) => {
                                                if let Some(search_items) = search_data.get("items").and_then(|i| i.as_array()) {
                                                    // Apply offset and limit manually since YouTube API doesn't support offset directly
                                                    let start_index = offset as usize;
                                                    let end_index = (offset + max_results) as usize;
                                                    
                                                    // Slice the results according to offset and limit
                                                    let paginated_items = if start_index < search_items.len() {
                                                        let actual_end = std::cmp::min(end_index, search_items.len());
                                                        &search_items[start_index..actual_end]
                                                    } else {
                                                        &[][..] // Empty slice if offset is beyond available items
                                                    };
                                                    
                                                    for video in paginated_items {
                                                        // Skip the original video
                                                        if let Some(vid) = video.get("id").and_then(|id| id.get("videoId")).and_then(|v| v.as_str()) {
                                                            if vid == video_id {
                                                                continue;
                                                            }
                                                            
                                                            if let Some(vinfo) = video.get("snippet") {
                                                                let title = vinfo.get("title")
                                                                    .and_then(|t| t.as_str())
                                                                    .unwrap_or("Unknown Title")
                                                                    .to_string();
                                                                
                                                                let author = vinfo.get("channelTitle")
                                                                    .and_then(|a| a.as_str())
                                                                    .unwrap_or("Unknown Author")
                                                                    .to_string();
                                                                
                                                                let channel_id = vinfo.get("channelId")
                                                                    .and_then(|id| id.as_str())
                                                                    .unwrap_or("")
                                                                    .to_string();

                                                                let published_at = vinfo.get("publishedAt")
                                                                    .and_then(|p| p.as_str())
                                                                    .unwrap_or("")
                                                                    .to_string();
                                                                
                                                                let thumbnail = format!("{}thumbnail/{}", config.server.mainurl.trim_end_matches('/'), vid);
                                                                
                                                                let channel_thumbnail = format!("{}channel_icon/{}", config.server.mainurl.trim_end_matches('/'), if channel_id.is_empty() { vid } else { channel_id.as_str() });
                                                                
                                                                let video_url = format!("{}get-ytvideo-info.php?video_id={}&quality={}", 
                                                                    config.server.mainurl, vid, config.video.default_quality);
                                                                
                                                                let final_url = if config.proxy.use_video_proxy {
                                                                    format!("{}video.proxy?url={}", config.server.mainurl, urlencoding::encode(&video_url))
                                                                } else {
                                                                    video_url
                                                                };
                                                                
                                                                // Get view count
                                                                let stats_url = format!(
                                                                    "https://www.googleapis.com/youtube/v3/videos?part=statistics&id={}&key={}",
                                                                    vid, apikey
                                                                );
                                                                
                                                                let mut view_count = "0".to_string();
                                                                match client.get(&stats_url).send().await {
                                                                    Ok(stats_resp) => {
                                                                        match stats_resp.json::<serde_json::Value>().await {
                                                                            Ok(stats_data) => {
                                                                                if let Some(stats_items) = stats_data.get("items").and_then(|i| i.as_array()) {
                                                                                    if !stats_items.is_empty() {
                                                                                        view_count = stats_items[0]
                                                                                            .get("statistics")
                                                                                            .and_then(|s| s.get("viewCount"))
                                                                                            .and_then(|v| v.as_str())
                                                                                            .unwrap_or("0")
                                                                                            .to_string();
                                                                                    }
                                                                                }
                                                                            }
                                                                            Err(_) => {}
                                                                        }
                                                                    }
                                                                    Err(_) => {}
                                                                }
                                                                
                                                                related_videos.push(RelatedVideo {
                                                                    title,
                                                                    author,
                                                                    video_id: vid.to_string(),
                                                                    views: view_count,
                                                                    published_at,
                                                                    thumbnail,
                                                                    channel_thumbnail,
                                                                    url: final_url,
                                                                    source: "search".to_string(),
                                                                });
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                crate::log::info!("Error parsing search data: {}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        crate::log::info!("Error fetching search data: {}", e);
                                    }
                                }
                            }
                        } else {
                            return HttpResponse::NotFound().json(serde_json::json!({
                                "error": "Видео не найдено."
                            }));
                        }
                    }
                }
                Err(e) => {
                    crate::log::info!("Error parsing video data: {}", e);
                    return HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Internal server error"
                    }));
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error fetching video data: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Internal server error"
            }));
        }
    }
    
    HttpResponse::Ok().json(related_videos)
}

#[utoipa::path(
    get,
    path = "/get-direct-video-url.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality")
    ),
    responses(
        (status = 200, description = "Direct URL for the video", body = DirectUrlResponse),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn get_direct_video_url(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID параметр обязателен"
            }));
        }
    };
    
    let quality = query_params.get("quality").map(|q| q.as_str());
    match resolve_direct_stream_url(&video_id, quality, false, &data.config).await {
        Some(url) => HttpResponse::Ok().json(DirectUrlResponse { video_url: url }),
        None => HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "Failed to resolve direct url"
        })),
    }
}

#[utoipa::path(
    get,
    path = "/direct_url",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality"),
        ("proxy" = Option<String>, Query, description = "Pass-through proxy (true/false)")
    ),
    responses(
        (status = 200, description = "Video stream"),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn direct_url(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID параметр обязателен"
            }));
        }
    };
    
    let quality = query_params.get("quality").map(|q| q.as_str());
    let proxy_param = query_params.get("proxy").map(|p| p.to_lowercase()).unwrap_or_else(|| "true".to_string());
    let use_proxy = proxy_param != "false";
    
    let direct_url = match resolve_direct_stream_url(&video_id, quality, false, &data.config).await {
        Some(url) => url,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to resolve video url"
            }));
        }
    };
    
    if req.method() == actix_web::http::Method::HEAD {
        let client = Client::new();
        match client.head(&direct_url).send().await {
            Ok(resp) => {
                let mut builder = HttpResponse::build(resp.status());
                if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                    builder.insert_header((CONTENT_LENGTH, len.clone()));
                }
                if let Some(range) = resp.headers().get(CONTENT_RANGE) {
                    builder.insert_header((CONTENT_RANGE, range.clone()));
                }
                builder.insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")));
                builder.finish()
            }
            Err(_) => HttpResponse::Ok().finish(),
        }
    } else if !use_proxy {
        HttpResponse::Found()
            .insert_header((LOCATION, direct_url))
            .finish()
    } else {
        proxy_stream_response(&direct_url, &req, "video/mp4").await
    }
}

#[utoipa::path(
    get,
    path = "/direct_audio_url",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("proxy" = Option<String>, Query, description = "Pass-through proxy (true/false)")
    ),
    responses(
        (status = 200, description = "Audio stream"),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn direct_audio_url(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID параметр обязателен"
            }));
        }
    };
    
    let proxy_param = query_params.get("proxy").map(|p| p.to_lowercase()).unwrap_or_else(|| "true".to_string());
    let use_proxy = proxy_param != "false";
    
    let direct_url = match resolve_direct_stream_url(&video_id, None, true, &data.config).await {
        Some(url) => url,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to resolve audio url"
            }));
        }
    };
    
    if req.method() == actix_web::http::Method::HEAD {
        let client = Client::new();
        match client.head(&direct_url).send().await {
            Ok(resp) => {
                let mut builder = HttpResponse::build(resp.status());
                if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                    builder.insert_header((CONTENT_LENGTH, len.clone()));
                }
                if let Some(range) = resp.headers().get(CONTENT_RANGE) {
                    builder.insert_header((CONTENT_RANGE, range.clone()));
                }
                builder.insert_header((CONTENT_TYPE, HeaderValue::from_static("audio/m4a")));
                builder.finish()
            }
            Err(_) => HttpResponse::Ok().finish(),
        }
    } else if !use_proxy {
        HttpResponse::Found()
            .insert_header((LOCATION, direct_url))
            .finish()
    } else {
        proxy_stream_response(&direct_url, &req, "audio/m4a").await
    }
}

#[utoipa::path(
    get,
    path = "/video.proxy",
    params(
        ("url" = String, Query, description = "Target URL to proxy")
    ),
    responses(
        (status = 200, description = "Proxied response")
    )
)]
pub async fn video_proxy(
    req: HttpRequest,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let url = match query_params.get("url") {
        Some(u) => {
            urlencoding::decode(u).unwrap_or_else(|_| u.into()).to_string()
        }
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Missing url parameter"
            }));
        }
    };
    
    if req.method() == actix_web::http::Method::HEAD {
        let client = Client::new();
        match client.head(&url).send().await {
            Ok(resp) => {
                let mut builder = HttpResponse::build(resp.status());
                if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                    builder.insert_header((CONTENT_LENGTH, len.clone()));
                }
                if let Some(ct) = resp.headers().get(CONTENT_TYPE) {
                    builder.insert_header((CONTENT_TYPE, ct.clone()));
                }
                builder.finish()
            }
            Err(_) => HttpResponse::Ok().finish(),
        }
    } else {
        proxy_stream_response(&url, &req, "application/octet-stream").await
    }
}

#[utoipa::path(
    get,
    path = "/download",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality")
    ),
    responses(
        (status = 302, description = "Redirect to downloadable stream")
    )
)]
pub async fn download_video(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let video_id = match query_params.get("video_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "ID параметр обязателен"
            }));
        }
    };
    
    let quality = query_params.get("quality").map(|q| q.as_str());
    let direct_url = match resolve_direct_stream_url(&video_id, quality, false, &data.config).await {
        Some(url) => url,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to resolve video url"
            }));
        }
    };
    
    if req.method() == actix_web::http::Method::HEAD {
        HttpResponse::Ok().finish()
    } else {
        HttpResponse::Found()
            .insert_header((LOCATION, direct_url))
            .insert_header(("Content-Disposition", format!("attachment; filename=\"{}.mp4\"", video_id)))
            .finish()
    }
}
