use actix_web::{web, HttpRequest, HttpResponse, Responder};
use html_escape::decode_html_entities;
use regex::Regex;
use reqwest::Client;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::config::Config;
use crate::routes::auth::AuthConfig;
use crate::routes::oauth::refresh_access_token;
use std::fs;
fn base_url(req: &HttpRequest, config: &crate::config::Config) -> String {
    if !config.server.main_url.is_empty() {
        return config.server.main_url.clone();
    }
    let info = req.connection_info();
    let scheme = info.scheme();
    let host = info.host();
    format!("{}://{}/", scheme, host.trim_end_matches('/'))
}

fn mask_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.len() <= 6 {
        return "***".to_string();
    }
    let (start, end) = trimmed.split_at(3);
    let suffix = &end[end.len().saturating_sub(2)..];
    format!("{}***{}", start, suffix)
}

fn clean_text(input: &str) -> String {
    let decoded = decode_html_entities(input).to_string();
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn generate_cpn() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let bytes = Uuid::new_v4().into_bytes();
    let mut out = String::with_capacity(16);
    for b in bytes.iter().take(16) {
        let idx = (*b as usize) % CHARSET.len();
        out.push(CHARSET[idx] as char);
    }
    out
}

async fn is_key_valid(client: &Client, key: &str) -> bool {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return false;
    }

    let url = format!(
        "https://www.googleapis.com/youtube/v3/videos?part=id&id=dQw4w9WgXcQ&key={}",
        trimmed
    );

    matches!(client.get(&url).send().await, Ok(resp) if resp.status().is_success())
}

#[utoipa::path(
    get,
    path = "/check_api_keys",
    responses(
        (status = 200, description = "API key health check")
    )
)]
pub async fn check_api_keys() -> impl Responder {
    let path = "config.yml";
    let mut config = match crate::config::Config::from_file(path) {
        Ok(c) => c,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": format!("Failed to load config: {}", e)
            }));
        }
    };

    if config.api.keys.active.is_empty() {
        return HttpResponse::Ok().json(serde_json::json!({
            "checked": 0,
            "failed": [],
            "message": "No api_keys configured"
        }));
    }

    let client = Client::new();
    let original_keys = config.api.keys.active.clone();
    let mut working_keys: Vec<String> = Vec::with_capacity(original_keys.len());
    let mut failed_keys: Vec<String> = Vec::new();
    let mut failed_set: HashSet<String> = HashSet::new();

    for key in original_keys.iter() {
        let normalized = key.trim().to_string();
        if normalized.is_empty() {
            if failed_set.insert(normalized.clone()) {
                failed_keys.push(normalized);
            }
            continue;
        }

        if is_key_valid(&client, &normalized).await {
            working_keys.push(normalized);
        } else if failed_set.insert(normalized.clone()) {
            failed_keys.push(normalized);
        }
    }

    let checked = original_keys.len();
    config.api.keys.active = working_keys;

    for failed in failed_keys.iter() {
        if !config
            .api
            .keys
            .disabled
            .iter()
            .any(|existing| existing == failed)
        {
            config.api.keys.disabled.push(failed.clone());
        }
    }

    if let Err(e) = config.persist(path) {
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e
        }));
    }

    let masked_failed: Vec<String> = failed_keys.iter().map(|k| mask_key(k)).collect();

    HttpResponse::Ok().json(serde_json::json!({
        "checked": checked,
        "failed": masked_failed,
        "active": config.api.keys.active.len()
    }))
}

#[utoipa::path(
    get,
    path = "/check_failed_api_keys",
    responses(
        (status = 200, description = "Re-check non-working API keys")
    )
)]
pub async fn check_failed_api_keys() -> impl Responder {
    let path = "config.yml";
    let mut config = match crate::config::Config::from_file(path) {
        Ok(c) => c,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": format!("Failed to load config: {}", e)
            }));
        }
    };

    if config.api.keys.disabled.is_empty() {
        return HttpResponse::Ok().json(serde_json::json!({
            "checked": 0,
            "message": "No non-working api_keys configured"
        }));
    }

    let client = Client::new();
    let mut revived_keys: Vec<String> = Vec::new();
    let mut still_failed_keys: Vec<String> = Vec::new();

    for key in config.api.keys.disabled.iter() {
        let normalized = key.trim().to_string();

        if normalized.is_empty() {
            still_failed_keys.push(normalized);
            continue;
        }

        if is_key_valid(&client, &normalized).await {
            revived_keys.push(normalized);
        } else {
            still_failed_keys.push(normalized);
        }
    }

    let mut active_keys = config.api.keys.active.clone();
    for revived in revived_keys.iter() {
        if !active_keys.iter().any(|existing| existing == revived) {
            active_keys.push(revived.clone());
        }
    }

    config.api.keys.active = active_keys;
    config.api.keys.disabled = still_failed_keys.clone();

    if let Err(e) = config.persist(path) {
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e
        }));
    }

    HttpResponse::Ok().json(serde_json::json!({
        "checked": revived_keys.len() + still_failed_keys.len(),
        "revived": revived_keys.iter().map(|k| mask_key(k)).collect::<Vec<_>>(),
        "still_failed": still_failed_keys.iter().map(|k| mask_key(k)).collect::<Vec<_>>(),
        "active": config.api.keys.active.len()
    }))
}

#[derive(Serialize, ToSchema)]
pub struct RecommendationItem {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
    pub duration: String,
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
    pub channel_thumbnail: String,
}

#[derive(Serialize, ToSchema)]
pub struct InstantItem {
    pub url: String,
}

#[derive(Serialize, ToSchema)]
pub struct InstantsResponse {
    pub instants: Vec<InstantItem>,
}

fn parse_recommendations(
    json_data: &serde_json::Value,
    max_videos: usize,
) -> Vec<RecommendationItem> {
    let mut videos = Vec::new();

    if let Some(contents) = json_data
        .get("contents")
        .and_then(|c| c.get("tvBrowseRenderer"))
        .and_then(|t| t.get("content"))
        .and_then(|c| c.get("tvSurfaceContentRenderer"))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("sectionListRenderer"))
        .and_then(|c| c.get("contents"))
        .and_then(|c| c.as_array())
    {
        for section in contents {
            if videos.len() >= max_videos {
                break;
            }
            if let Some(items) = section
                .get("shelfRenderer")
                .and_then(|s| s.get("content"))
                .and_then(|c| c.get("horizontalListRenderer"))
                .and_then(|h| h.get("items"))
                .and_then(|i| i.as_array())
            {
                for item in items {
                    if videos.len() >= max_videos {
                        break;
                    }
                    if let Some(tile) = item.get("tileRenderer") {
                        if let Some(video_id) = tile
                            .get("onSelectCommand")
                            .and_then(|c| c.get("watchEndpoint"))
                            .and_then(|w| w.get("videoId"))
                            .and_then(|v| v.as_str())
                        {
                            let raw_title = tile
                                .get("metadata")
                                .and_then(|m| m.get("tileMetadataRenderer"))
                                .and_then(|t| t.get("title"))
                                .and_then(|t| t.get("simpleText"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("No Title");
                            let title = clean_text(raw_title);

                            let mut author = "Unknown".to_string();
                            if let Some(lines) = tile
                                .get("metadata")
                                .and_then(|m| m.get("tileMetadataRenderer"))
                                .and_then(|t| t.get("lines"))
                                .and_then(|l| l.as_array())
                            {
                                if let Some(first_line) = lines.get(0) {
                                    if let Some(text) = first_line
                                        .get("lineRenderer")
                                        .and_then(|l| l.get("items"))
                                        .and_then(|i| i.as_array())
                                        .and_then(|arr| arr.get(0))
                                        .and_then(|line_item| {
                                            line_item
                                                .get("lineItemRenderer")
                                                .and_then(|li| li.get("text"))
                                                .and_then(|t| t.get("runs"))
                                                .and_then(|r| r.as_array())
                                                .and_then(|r| r.get(0))
                                                .and_then(|r| r.get("text"))
                                                .and_then(|t| t.as_str())
                                        })
                                    {
                                        author = clean_text(text);
                                    }
                                }
                            }

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

                            videos.push(RecommendationItem {
                                title,
                                author,
                                video_id: video_id.to_string(),
                                thumbnail: String::new(),
                                channel_thumbnail: String::new(),
                                duration,
                            });
                        }
                    }
                }
            }
        }
    }

    videos
}

async fn fetch_history_page(
    access_token: &str,
    continuation: Option<String>,
    config: &crate::config::Config,
) -> Option<serde_json::Value> {
    let client = Client::new();
    let mut payload = serde_json::json!({
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
    if let Some(cont) = continuation {
        payload["continuation"] = serde_json::Value::String(cont);
    }
    let url = format!(
        "https://www.youtube.com/youtubei/v1/browse?key={}",
        config.get_api_key_rotated()
    );
    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&payload)
        .send()
        .await
        .ok()?;
    res.json::<serde_json::Value>().await.ok()
}

fn find_continuation_token(json_data: &serde_json::Value) -> Option<String> {
    if let Some(token) = json_data
        .get("continuationContents")
        .and_then(|c| c.get("gridContinuation"))
        .and_then(|g| g.get("continuations"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.get(0))
        .and_then(|c| c.get("nextContinuationData"))
        .and_then(|n| n.get("continuation"))
        .and_then(|c| c.as_str())
    {
        return Some(token.to_string());
    }
    if let Some(actions) = json_data
        .get("onResponseReceivedActions")
        .and_then(|a| a.as_array())
    {
        for action in actions {
            if let Some(items) = action
                .get("appendContinuationItemsAction")
                .and_then(|a| a.get("items"))
                .and_then(|i| i.as_array())
            {
                for item in items {
                    if let Some(token) = item
                        .get("continuationItemRenderer")
                        .and_then(|c| c.get("continuationEndpoint"))
                        .and_then(|e| e.get("continuationCommand"))
                        .and_then(|c| c.get("token"))
                        .and_then(|t| t.as_str())
                    {
                        return Some(token.to_string());
                    }
                }
            }
        }
    }
    None
}

fn parse_history_tile(tile: &serde_json::Value, base_trimmed: &str) -> Option<HistoryItem> {
    let video_id = tile
        .get("onSelectCommand")
        .and_then(|c| c.get("watchEndpoint"))
        .and_then(|w| w.get("videoId"))
        .and_then(|v| v.as_str())?;
    let raw_title = tile
        .get("metadata")
        .and_then(|m| m.get("tileMetadataRenderer"))
        .and_then(|t| t.get("title"))
        .and_then(|t| t.get("simpleText"))
        .and_then(|t| t.as_str())
        .unwrap_or("No Title");
    let title = clean_text(raw_title);
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
    let watched_at = tile
        .get("metadata")
        .and_then(|m| m.get("tileMetadataRenderer"))
        .and_then(|t| t.get("lines"))
        .and_then(|l| l.as_array())
        .and_then(|arr| arr.get(1))
        .and_then(|line| line.get("lineRenderer"))
        .and_then(|l| l.get("items"))
        .and_then(|i| i.as_array())
        .and_then(|arr| arr.get(2))
        .and_then(|li| li.get("lineItemRenderer"))
        .and_then(|l| l.get("text"))
        .and_then(|t| t.get("simpleText"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    Some(HistoryItem {
        video_id: video_id.to_string(),
        title,
        author,
        views: "0".to_string(),
        duration,
        watched_at,
        thumbnail: format!("{}/thumbnail/{}", base_trimmed, video_id),
        channel_thumbnail: String::new(),
    })
}

fn extract_history_data_with_continuation(
    json_data: serde_json::Value,
    max_videos: usize,
    base_trimmed: &str,
) -> (Vec<HistoryItem>, Option<String>) {
    let mut videos = Vec::new();
    let mut continuation = find_continuation_token(&json_data);

    if let Some(contents) = json_data
        .get("contents")
        .and_then(|c| c.get("tvBrowseRenderer"))
        .and_then(|t| t.get("content"))
        .and_then(|c| c.get("tvSurfaceContentRenderer"))
        .and_then(|c| c.get("content"))
    {
        if let Some(items) = contents
            .get("gridRenderer")
            .and_then(|g| g.get("items"))
            .and_then(|i| i.as_array())
        {
            for item in items {
                if videos.len() >= max_videos {
                    break;
                }
                if let Some(tile) = item.get("tileRenderer") {
                    if let Some(parsed) = parse_history_tile(tile, base_trimmed) {
                        videos.push(parsed);
                    }
                }
            }
        }
        if videos.len() < max_videos {
            if let Some(actions) = json_data
                .get("onResponseReceivedActions")
                .and_then(|a| a.as_array())
            {
                for action in actions {
                    if let Some(items) = action
                        .get("appendContinuationItemsAction")
                        .and_then(|a| a.get("items"))
                        .and_then(|i| i.as_array())
                    {
                        for item in items {
                            if videos.len() >= max_videos {
                                break;
                            }
                            if let Some(tile) = item.get("tileRenderer") {
                                if let Some(parsed) = parse_history_tile(tile, base_trimmed) {
                                    videos.push(parsed);
                                }
                            }
                            if continuation.is_none() {
                                continuation = item
                                    .get("continuationItemRenderer")
                                    .and_then(|c| c.get("continuationEndpoint"))
                                    .and_then(|e| e.get("continuationCommand"))
                                    .and_then(|c| c.get("token"))
                                    .and_then(|t| t.as_str())
                                    .map(|s| s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    (videos, continuation)
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
    let base = base_url(&req, &data.config);
    let base_trimmed = base.trim_end_matches('/');
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

    let count: usize = query_params
        .get("count")
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

    let api_key = match data.config.get_innertube_key() {
        Some(k) => k,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };

    let url = format!("https://www.youtube.com/youtubei/v1/browse?key={}", api_key);

    let res = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&payload)
        .send()
        .await;

    match res {
        Ok(response) => match response.json::<serde_json::Value>().await {
            Ok(json_data) => {
                let mut recommendations = parse_recommendations(&json_data, count);
                for item in &mut recommendations {
                    item.thumbnail = format!("{}/thumbnail/{}", base_trimmed, item.video_id);
                }
                HttpResponse::Ok().json(recommendations)
            }
            Err(e) => {
                crate::log::info!("Error parsing recommendations: {}", e);
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": "Failed to parse response"
                }))
            }
        },
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
    let base = base_url(&req, &data.config);
    let base_trimmed = base.trim_end_matches('/');
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

    let url = format!(
        "https://www.youtube.com/youtubei/v1/browse?key={}",
        data.config.get_api_key_rotated()
    );

    let res = client
        .post(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&payload)
        .send()
        .await;

    match res {
        Ok(response) => match response.json::<serde_json::Value>().await {
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

                                let mut thumb_url = thumb_url.to_string();
                                if thumb_url.starts_with("//") {
                                    thumb_url = format!("https:{}", thumb_url);
                                }
                                let encoded_thumb = urlencoding::encode(&thumb_url);

                                subs.push(SubscriptionItem {
                                    channel_id: channel_id.to_string(),
                                    title: username.to_string(),
                                    thumbnail: thumb_url.to_string(),
                                    local_thumbnail: format!("{}/channel_icon/{}", base_trimmed, encoded_thumb),
                                    profile_url: format!("{}get_author_videos.php?author={}", base_trimmed, username),
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
        },
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
        (status = 200, description = "Watch history", body = [HistoryItem]),
        (status = 400, description = "Missing token")
    )
)]
pub async fn get_history(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let base = base_url(&req, &data.config);
    let base_trimmed = base.trim_end_matches('/');
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

    let count: usize = query_params
        .get("count")
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

    let mut videos: Vec<HistoryItem> = Vec::new();
    let mut continuation: Option<String> = None;

    while videos.len() < count {
        let page = fetch_history_page(&access_token, continuation.clone(), &data.config).await;
        if page.is_none() {
            break;
        }
        let (mut page_items, next) = extract_history_data_with_continuation(
            page.unwrap(),
            count - videos.len(),
            base_trimmed,
        );
        videos.append(&mut page_items);
        if next.is_none() {
            break;
        }
        continuation = next;
    }

    HttpResponse::Ok().json(videos)
}

fn extract_feedback_token(player_body: &str) -> Option<String> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(player_body) {
        if let Some(url) = json
            .pointer("/playbackTracking/videostatsPlaybackUrl/baseUrl")
            .and_then(|v| v.as_str())
        {
            return Some(url.to_string());
        }

        if let Some(token) = json
            .pointer("/playbackTracking/videostatsPlaybackUrl/feedbackToken")
            .and_then(|v| v.as_str())
        {
            return Some(token.to_string());
        }

        if let Some(token) = json
            .get("feedbackTokens")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.get(0))
            .and_then(|v| v.as_str())
        {
            return Some(token.to_string());
        }
    }

    Regex::new(r#""feedbackToken"\s*:\s*"([^"]+)""#)
        .ok()
        .and_then(|re| re.captures(player_body))
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
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

    let api_key = match data.config.get_innertube_key() {
        Some(k) => k,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };
    let client = Client::new();
    let cpn = generate_cpn();
    let user_agent = "com.google.android.youtube/19.14.37";

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

    let build_payload = |include_params: bool| {
        let mut payload = serde_json::json!({
            "videoId": video_id,
            "cpn": cpn,
            "context": context["context"],
            "contentCheckOk": true,
            "racyCheckOk": true
        });
        if include_params {
            payload["params"] = serde_json::json!("CgIIAQ==");
        }
        payload
    };

    let mut player_body = String::new();
    let mut player_ok = false;

    for include_params in [false, true] {
        let player_payload = build_payload(include_params);
        let resp = client
            .post(&format!(
                "https://www.youtube.com/youtubei/v1/player?key={}",
                api_key
            ))
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("User-Agent", user_agent)
            .json(&player_payload)
            .send()
            .await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                crate::log::info!("Player request failed: {}", e);
                continue;
            }
        };

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status.is_success() {
            player_body = body;
            player_ok = true;
            break;
        } else {
            let snippet: String = body.chars().take(300).collect();
            crate::log::info!(
                "Player attempt (params={}): status {} body {}",
                include_params,
                status,
                snippet
            );
            player_body = snippet;
        }
    }

    if !player_ok {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Player request failed",
            "details": player_body
        }));
    }

    let feedback_token = match extract_feedback_token(&player_body) {
        Some(token) => token,
        None => {
            crate::log::info!("No feedback token found in player response");
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to find feedback token"
            }));
        }
    };

    let feedback_payload = serde_json::json!({
        "context": context["context"],
        "feedbackTokens": [feedback_token]
    });

    let feedback_resp = client
        .post(&format!(
            "https://www.youtube.com/youtubei/v1/feedback?key={}",
            api_key
        ))
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .header("User-Agent", user_agent)
        .json(&feedback_payload)
        .send()
        .await;

    match feedback_resp {
        Ok(resp) if resp.status().is_success() => HttpResponse::Ok().json(serde_json::json!({
            "status": "success",
            "message": format!("Video {} marked as watched", video_id)
        })),
        Ok(resp) => {
            let snippet = resp.text().await.unwrap_or_default();
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Feedback request failed",
                "details": snippet.chars().take(300).collect::<String>()
            }))
        }
        Err(e) => {
            crate::log::info!("Feedback request error: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to send feedback request"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/get-instants",
    responses(
        (status = 200, description = "List of available instances", body = InstantsResponse)
    )
)]
pub async fn get_instants(data: web::Data<crate::AppState>) -> impl Responder {
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
        instants: instants
            .into_iter()
            .map(|i| InstantItem { url: i.0 })
            .collect(),
    };

    HttpResponse::Ok().json(response)
}
