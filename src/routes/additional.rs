use actix_web::{web, HttpRequest, HttpResponse, Responder};
use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::routes::auth::AuthConfig;
use crate::config::Config;
use std::fs;

#[derive(Serialize, ToSchema)]
pub struct RecommendationItem {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
}

#[derive(Serialize, ToSchema)]
pub struct SubscriptionItem {
    pub channel_id: String,
    pub title: String,
    pub thumbnail: String,
    pub local_thumbnail: String,
    pub profile_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct SubscriptionsResponse {
    pub status: String,
    pub count: usize,
    pub subscriptions: Vec<SubscriptionItem>,
}

#[derive(Serialize, ToSchema)]
pub struct HistoryItem {
    pub video_id: String,
    pub title: String,
    pub author: String,
    pub views: String,
    pub duration: String,
    pub watched_at: String,
    pub thumbnail: String,
}

#[derive(Serialize, ToSchema)]
pub struct HistoryResponse {
    pub items: Vec<HistoryItem>,
}

#[derive(Serialize, ToSchema)]
pub struct InstantItem {
    pub url: String,
}

#[derive(Serialize, ToSchema)]
pub struct InstantsResponse {
    pub instants: Vec<InstantItem>,
}

async fn refresh_access_token(
    refresh_token: &str,
    auth_config: &AuthConfig,
) -> Result<String, String> {
    let client = Client::new();
    let params = [
        ("client_id", auth_config.client_id.as_str()),
        ("client_secret", auth_config.client_secret.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    
    let res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    
    if !res.status().is_success() {
        return Err(format!("Token refresh failed: {}", res.status()));
    }
    
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if let Some(access) = json.get("access_token").and_then(|t| t.as_str()) {
        Ok(access.to_string())
    } else {
        Err("No access_token in response".to_string())
    }
}

fn parse_recommendations(json_data: &serde_json::Value, max_videos: usize) -> Vec<RecommendationItem> {
    let mut videos = Vec::new();
    
    if let Some(contents) = json_data
        .get("contents")
        .and_then(|c| c.get("tvBrowseRenderer"))
        .and_then(|t| t.get("content"))
        .and_then(|c| c.get("tvSurfaceContentRenderer"))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("sectionListRenderer"))
        .and_then(|c| c.get("contents"))
        .and_then(|c| c.as_array()) {
        
        for section in contents {
            if videos.len() >= max_videos {
                break;
            }
            if let Some(items) = section
                .get("shelfRenderer")
                .and_then(|s| s.get("content"))
                .and_then(|c| c.get("horizontalListRenderer"))
                .and_then(|h| h.get("items"))
                .and_then(|i| i.as_array()) {
                
                for item in items {
                    if videos.len() >= max_videos {
                        break;
                    }
                    if let Some(tile) = item.get("tileRenderer") {
                        if let Some(video_id) = tile
                            .get("onSelectCommand")
                            .and_then(|c| c.get("watchEndpoint"))
                            .and_then(|w| w.get("videoId"))
                            .and_then(|v| v.as_str()) {
                            
                            let title = tile
                                .get("metadata")
                                .and_then(|m| m.get("tileMetadataRenderer"))
                                .and_then(|t| t.get("title"))
                                .and_then(|t| t.get("simpleText"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("No Title")
                                .to_string();
                            
                            let mut author = "Unknown".to_string();
                            if let Some(lines) = tile
                                .get("metadata")
                                .and_then(|m| m.get("tileMetadataRenderer"))
                                .and_then(|t| t.get("lines"))
                                .and_then(|l| l.as_array()) {
                                if let Some(first_line) = lines.get(0) {
                                    if let Some(text) = first_line
                                        .get("lineRenderer")
                                        .and_then(|l| l.get("items"))
                                        .and_then(|i| i.as_array())
                                        .and_then(|arr| arr.get(0))
                                        .and_then(|line_item| line_item
                                            .get("lineItemRenderer")
                                            .and_then(|li| li.get("text"))
                                            .and_then(|t| t.get("runs"))
                                            .and_then(|r| r.as_array())
                                            .and_then(|r| r.get(0))
                                            .and_then(|r| r.get("text"))
                                            .and_then(|t| t.as_str())) {
                                        author = text.to_string();
                                    }
                                }
                            }
                            
                            videos.push(RecommendationItem {
                                title,
                                author,
                                video_id: video_id.to_string(),
                                thumbnail: String::new(),
                                channel_thumbnail: String::new(),
                            });
                        }
                    }
                }
            }
        }
    }
    
    videos
}

#[utoipa::path(
    get,
    path = "/get_recommendations.php",
    params(
        ("token" = String, Query, description = "Refresh token"),
        ("count" = Option<i32>, Query, description = "How many recommendations to return (default: 50)")
    ),
    responses(
        (status = 200, description = "Recommendations list", body = [RecommendationItem]),
        (status = 400, description = "Missing token")
    )
)]
pub async fn get_recommendations(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let refresh_token = match query_params.get("token") {
        Some(t) => t.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Missing token parameter. Use ?token=YOUR_REFRESH_TOKEN"
            }));
        }
    };
    
    let count: usize = query_params.get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(data.config.video.default_count as usize);
    
    let access_token = match refresh_access_token(&refresh_token, &auth_config) .await {
        Ok(t) => t,
        Err(e) => {
            return HttpResponse::Unauthorized().json(serde_json::json!({
                "error": "Invalid refresh token",
                "details": e
            }));
        }
    };
    
    let client = Client::new();
    let payload = serde_json::json!({
        "context": {
            "client": {
                "hl": "en",
                "gl": "US",
                "deviceMake": "Samsung",
                "deviceModel": "SmartTV",
                "userAgent": "Mozilla/5.0 (SMART-TV; Linux; Tizen 5.0) AppleWebKit/538.1",
                "clientName": "TVHTML5",
                "clientVersion": "7.20250209.19.00",
                "osName": "Tizen",
                "osVersion": "5.0",
                "platform": "TV",
                "clientFormFactor": "UNKNOWN_FORM_FACTOR",
                "screenPixelDensity": 1
            }
        },
        "browseId": "FEwhat_to_watch"
    });
    
    let url = format!(
        "https://www.youtube.com/youtubei/v1/browse?key={}",
        data.config.get_api_key_rotated()
    );
    
    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&payload)
        .send()
        .await;
    
    match res {
        Ok(response) => {
            match response.json::<serde_json::Value>().await {
                Ok(json_data) => {
                    let mut recommendations = parse_recommendations(&json_data, count);
                    // enrich thumbnails
                    for item in &mut recommendations {
                        item.thumbnail = format!("{}thumbnail/{}", data.config.server.mainurl.trim_end_matches('/'), item.video_id);
                    }
                    HttpResponse::Ok().json(recommendations)
                }
                Err(e) => {
                    crate::log::info!("Error parsing recommendations: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to parse response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling recommendations API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call recommendations API"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/get_subscriptions.php",
    params(
        ("token" = String, Query, description = "Refresh token")
    ),
    responses(
        (status = 200, description = "Subscriptions list", body = SubscriptionsResponse),
        (status = 400, description = "Missing token")
    )
)]
pub async fn get_subscriptions(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let refresh_token = match query_params.get("token") {
        Some(t) => t.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Missing token parameter. Use ?token=YOUR_REFRESH_TOKEN"
            }));
        }
    };
    
    let access_token = match refresh_access_token(&refresh_token, &auth_config).await {
        Ok(t) => t,
        Err(e) => {
            return HttpResponse::Unauthorized().json(serde_json::json!({
                "error": "Invalid refresh token",
                "details": e
            }));
        }
    };
    
    let client = Client::new();
    let payload = serde_json::json!({
        "context": {
            "client": {
                "hl": "en", "gl": "US", "deviceMake": "Samsung", "deviceModel": "SmartTV",
                "userAgent": "Mozilla/5.0 (SMART-TV; Linux; Tizen 5.0) AppleWebKit/538.1",
                "clientName": "TVHTML5", "clientVersion": "7.20250209.19.00",
                "osName": "Tizen", "osVersion": "5.0", "platform": "TV",
                "clientFormFactor": "UNKNOWN_FORM_FACTOR", "screenPixelDensity": 1
            }
        },
        "browseId": "FEsubscriptions"
    });
    
    let url = "https://www.youtube.com/youtubei/v1/browse?key=AIzaSyDCU8hByM-4DrUqRUYnGn-3llEO78bcxq8";
    
    let res = client
        .post(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&payload)
        .send()
        .await;
    
    match res {
        Ok(response) => {
            match response.json::<serde_json::Value>().await {
                Ok(json_data) => {
                    let mut subs = Vec::new();
                    if let Some(tabs) = json_data.pointer("/contents/tvBrowseRenderer/content/tvSecondaryNavRenderer/sections/0/tvSecondaryNavSectionRenderer/tabs")
                        .and_then(|t| t.as_array()) {
                        for tab in tabs {
                            if let Some(renderer) = tab.get("tabRenderer") {
                                let username = renderer.get("title").and_then(|t| t.as_str()).unwrap_or("Unknown");
                                if username.eq_ignore_ascii_case("all") {
                                    continue;
                                }
                                let thumb_url = renderer
                                    .get("thumbnail")
                                    .and_then(|t| t.get("thumbnails"))
                                    .and_then(|th| th.as_array())
                                    .and_then(|arr| arr.last())
                                    .and_then(|v| v.get("url"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let channel_id = renderer
                                    .get("endpoint")
                                    .and_then(|e| e.get("browseEndpoint"))
                                    .and_then(|b| b.get("browseId"))
                                    .and_then(|b| b.as_str())
                                    .unwrap_or("unknown");
                                
                                subs.push(SubscriptionItem {
                                    channel_id: channel_id.to_string(),
                                    title: username.to_string(),
                                    thumbnail: thumb_url.to_string(),
                                    local_thumbnail: format!("{}channel_icon/{}", data.config.server.mainurl.trim_end_matches('/'), thumb_url),
                                    profile_url: format!("{}get_author_videos.php?author={}", data.config.server.mainurl, username),
                                });
                            }
                        }
                    }
                    
                    HttpResponse::Ok().json(SubscriptionsResponse {
                        status: "success".to_string(),
                        count: subs.len(),
                        subscriptions: subs,
                    })
                }
                Err(e) => {
                    crate::log::info!("Error parsing subscriptions: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to parse response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling subscriptions API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call subscriptions API"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/get_history.php",
    params(
        ("token" = String, Query, description = "Refresh token"),
        ("count" = Option<i32>, Query, description = "Number of videos to return (default: 50)")
    ),
    responses(
        (status = 200, description = "Watch history", body = HistoryResponse),
        (status = 400, description = "Missing token")
    )
)]
pub async fn get_history(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let refresh_token = match query_params.get("token") {
        Some(t) => t.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Missing token parameter"
            }));
        }
    };
    
    let count: usize = query_params.get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(data.config.video.default_count as usize);
    
    let access_token = match refresh_access_token(&refresh_token, &auth_config).await {
        Ok(t) => t,
        Err(e) => {
            return HttpResponse::Unauthorized().json(serde_json::json!({
                "error": "Invalid refresh token",
                "details": e
            }));
        }
    };
    
    let client = Client::new();
    let payload = serde_json::json!({
        "context": {
            "client": {
                "hl": "en", "gl": "US", "deviceMake": "Samsung", "deviceModel": "SmartTV",
                "userAgent": "Mozilla/5.0 (SMART-TV; Linux; Tizen 5.0) AppleWebKit/538.1",
                "clientName": "TVHTML5", "clientVersion": "7.20250209.19.00",
                "osName": "Tizen", "osVersion": "5.0", "platform": "TV",
                "clientFormFactor": "UNKNOWN_FORM_FACTOR", "screenPixelDensity": 1
            }
        },
        "browseId": "FEhistory"
    });
    
    let url = format!(
        "https://www.youtube.com/youtubei/v1/browse?key={}",
        data.config.get_api_key_rotated()
    );
    
    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&payload)
        .send()
        .await;
    
    match res {
        Ok(response) => {
            match response.json::<serde_json::Value>().await {
                Ok(json_data) => {
                    let mut items = Vec::new();
                    if let Some(contents) = json_data
                        .get("contents")
                        .and_then(|c| c.get("tvBrowseRenderer"))
                        .and_then(|t| t.get("content"))
                        .and_then(|c| c.get("tvSurfaceContentRenderer"))
                        .and_then(|c| c.get("content"))
                        .and_then(|c| c.get("gridRenderer"))
                        .and_then(|g| g.get("items"))
                        .and_then(|i| i.as_array()) {
                        for item in contents.iter().take(count) {
                            if let Some(tile) = item.get("tileRenderer") {
                                if let Some(video_id) = tile
                                    .get("onSelectCommand")
                                    .and_then(|c| c.get("watchEndpoint"))
                                    .and_then(|w| w.get("videoId"))
                                    .and_then(|v| v.as_str()) {
                                    
                                    let title = tile
                                        .get("metadata")
                                        .and_then(|m| m.get("tileMetadataRenderer"))
                                        .and_then(|t| t.get("title"))
                                        .and_then(|t| t.get("simpleText"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("No Title")
                                        .to_string();
                                    
                                    let author = "Unknown".to_string();
                                    
                                    let duration = tile
                                        .get("header")
                                        .and_then(|h| h.get("tileHeaderRenderer"))
                                        .and_then(|t| t.get("thumbnailOverlays"))
                                        .and_then(|o| o.as_array())
                                        .and_then(|arr| arr.get(0))
                                        .and_then(|o| o.get("thumbnailOverlayTimeStatusRenderer"))
                                        .and_then(|t| t.get("text"))
                                        .and_then(|t| t.get("simpleText"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("0:00")
                                        .to_string();
                                    
                                    items.push(HistoryItem {
                                        video_id: video_id.to_string(),
                                        title,
                                        author,
                                        views: "0".to_string(),
                                        duration,
                                        watched_at: "".to_string(),
                                        thumbnail: format!("{}thumbnail/{}", data.config.server.mainurl.trim_end_matches('/'), video_id),
                                    });
                                }
                            }
                        }
                    }
                    
                    HttpResponse::Ok().json(HistoryResponse { items })
                }
                Err(e) => {
                    crate::log::info!("Error parsing history: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to parse response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling history API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call history API"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/mark_video_watched.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("token" = String, Query, description = "Refresh token")
    ),
    responses(
        (status = 200, description = "Marked as watched"),
        (status = 400, description = "Missing parameters")
    )
)]
pub async fn mark_video_watched(
    req: HttpRequest,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let video_id = match query_params.get("video_id") {
        Some(v) => v.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Missing video_id"
            }));
        }
    };
    
    let refresh_token = match query_params.get("token") {
        Some(t) => t.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Missing token"
            }));
        }
    };
    
    let access_token = match refresh_access_token(&refresh_token, &auth_config).await {
        Ok(t) => t,
        Err(e) => {
            return HttpResponse::Unauthorized().json(serde_json::json!({
                "error": "Invalid refresh token",
                "details": e
            }));
        }
    };
    
    let client = Client::new();
    let api_key = "AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8";
    let cpn = Uuid::new_v4().to_string().replace('-', "");
    let cpn = cpn.chars().take(16).collect::<String>();
    
    let context = serde_json::json!({
        "context": {
            "client": {
                "clientName": "ANDROID",
                "clientVersion": "19.14.37",
                "hl": "en",
                "gl": "US",
                "osName": "Android",
                "osVersion": "13",
                "platform": "MOBILE"
            }
        }
    });
    
    let player_payload = serde_json::json!({
        "videoId": video_id,
        "cpn": cpn,
        "context": context["context"]
    });
    
    let player_resp = client
        .post(&format!("https://www.youtube.com/youtubei/v1/player?key={}", api_key))
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&player_payload)
        .send()
        .await;
    
    if let Ok(resp) = player_resp {
        if resp.status().is_success() {
            // best-effort feedback
            let feedback_payload = serde_json::json!({
                "context": context["context"],
                "feedbackTokens": []
            });
            let _ = client
                .post(&format!("https://www.youtube.com/youtubei/v1/feedback?key={}", api_key))
                .header("Authorization", format!("Bearer {}", access_token))
                .json(&feedback_payload)
                .send()
                .await;
            
            return HttpResponse::Ok().json(serde_json::json!({
                "status": "success",
                "message": format!("Video {} marked as watched", video_id)
            }));
        }
    }
    
    HttpResponse::InternalServerError().json(serde_json::json!({
        "error": "Failed to mark video as watched"
    }))
}

#[utoipa::path(
    get,
    path = "/get-instants",
    responses(
        (status = 200, description = "List of available instances", body = InstantsResponse)
    )
)]
pub async fn get_instants(
    data: web::Data<crate::AppState>,
) -> impl Responder {
    // Живое обновление: пытаемся перечитать config.yml на каждый запрос.
    let instants = match fs::read_to_string("config.yml") {
        Ok(contents) => {
            if let Ok(parsed) = serde_yaml::from_str::<Config>(&contents) {
                parsed.instants
            } else {
                data.config.instants.clone()
            }
        }
        Err(_) => data.config.instants.clone(),
    };
    
    let response = InstantsResponse {
        instants: instants.into_iter().map(|i| InstantItem { url: i.url }).collect(),
    };
    
    HttpResponse::Ok().json(response)
}
