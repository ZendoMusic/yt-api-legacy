use actix_web::{web, HttpResponse, Responder, HttpRequest};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use lru::LruCache;
use tokio::sync::Mutex;
use lazy_static::lazy_static;
use serde::Serialize;
use utoipa::ToSchema;
use chrono::DateTime;

lazy_static! {
    static ref THUMBNAIL_CACHE: Arc<Mutex<LruCache<String, (Vec<u8>, String, u64)>>> = 
        Arc::new(Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1000).unwrap())));
}

const CACHE_DURATION: u64 = 3600;

#[derive(Serialize, ToSchema)]
pub struct VideoInfoResponse {
    pub title: String,
    pub author: String,
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
                                                    format!("{}channel_icon/{}", config.server.mainurl.trim_end_matches('/'), video_id)
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
                        channel_thumbnail: format!("{}channel_icon/{}", config.server.mainurl.trim_end_matches('/'), video_id),
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
