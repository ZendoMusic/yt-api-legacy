use actix_web::{web, HttpResponse, Responder, HttpRequest};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use lru::LruCache;
use tokio::sync::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    static ref THUMBNAIL_CACHE: Arc<Mutex<LruCache<String, (Vec<u8>, String, u64)>>> = 
        Arc::new(Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1000).unwrap())));
}

const CACHE_DURATION: u64 = 3600;

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