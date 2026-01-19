use actix_web::{web, HttpRequest, HttpResponse, Responder};
use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use urlencoding;
use utoipa::ToSchema;

fn base_url(req: &HttpRequest, config: &crate::config::Config) -> String {
    if !config.server.main_url.is_empty() {
        return config.server.main_url.clone();
    }
    let info = req.connection_info();
    let scheme = info.scheme();
    let host = info.host();
    format!("{}://{}/", scheme, host.trim_end_matches('/'))
}

#[derive(Serialize, ToSchema)]
pub struct ChannelInfo {
    pub title: String,
    pub description: String,
    pub thumbnail: String,
    pub banner: String,
    pub subscriber_count: String,
    pub video_count: String,
}

#[derive(Serialize, ToSchema)]
pub struct ChannelVideo {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
    pub views: String,
    pub published_at: String,
    pub duration: String,
}

#[derive(Serialize, ToSchema)]
pub struct ChannelVideosResponse {
    pub channel_info: ChannelInfo,
    pub videos: Vec<ChannelVideo>,
}

async fn fetch_channel_thumbnail(channel_id: &str, apikey: &str) -> Option<String> {
    let client = Client::new();
    let url = format!(
        "https://www.googleapis.com/youtube/v3/channels?part=snippet&id={}&key={}",
        channel_id, apikey
    );

    if let Ok(resp) = client.get(&url).send().await {
        if let Ok(data) = resp.json::<serde_json::Value>().await {
            if let Some(items) = data.get("items").and_then(|i| i.as_array()) {
                if let Some(snippet) = items.get(0).and_then(|i| i.get("snippet")) {
                    if let Some(url) = snippet
                        .get("thumbnails")
                        .and_then(|t| t.get("high"))
                        .and_then(|h| h.get("url"))
                        .and_then(|u| u.as_str())
                    {
                        return Some(url.to_string());
                    }
                }
            }
        }
    }

    None
}

async fn fetch_channel_videos(
    channel_id: &str,
    count: i32,
    apikey: &str,
    _config: &crate::config::Config,
    base: &str,
) -> (Vec<ChannelVideo>, ChannelInfo) {
    let client = Client::new();

    fn parse_iso_duration(iso: &str) -> String {
        let mut hours = 0;
        let mut minutes = 0;
        let mut seconds = 0;
        let mut number = String::new();
        for ch in iso.chars() {
            if ch.is_ascii_digit() {
                number.push(ch);
            } else {
                let val = number.parse::<u64>().unwrap_or(0);
                match ch {
                    'H' => hours = val,
                    'M' => minutes = val,
                    'S' => seconds = val,
                    _ => {}
                }
                number.clear();
            }
        }
        if hours > 0 {
            format!("{}:{:02}:{:02}", hours, minutes, seconds)
        } else {
            format!("{}:{:02}", minutes, seconds)
        }
    }

    let channel_url = format!(
        "https://www.googleapis.com/youtube/v3/channels?part=snippet,statistics,brandingSettings&id={}&key={}",
        channel_id, apikey
    );

    let channel_resp = client.get(&channel_url).send().await;
    let channel_data: serde_json::Value = match channel_resp {
        Ok(resp) => resp.json().await.unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };

    let channel_info_value = channel_data
        .get("items")
        .and_then(|i| i.as_array())
        .and_then(|arr| arr.get(0))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let channel_title = channel_info_value
        .get("snippet")
        .and_then(|s| s.get("title"))
        .and_then(|t| t.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let _channel_thumb = fetch_channel_thumbnail(channel_id, apikey)
        .await
        .unwrap_or_default();

    let banner = channel_info_value
        .get("brandingSettings")
        .and_then(|b| b.get("image"))
        .and_then(|i| i.get("bannerExternalUrl"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let banner = if banner.starts_with("//") {
        format!("https:{}", banner)
    } else {
        banner
    };

    let subscriber_count = channel_info_value
        .get("statistics")
        .and_then(|s| s.get("subscriberCount"))
        .and_then(|v| v.as_str())
        .unwrap_or("0")
        .to_string();

    let video_count = channel_info_value
        .get("statistics")
        .and_then(|s| s.get("videoCount"))
        .and_then(|v| v.as_str())
        .unwrap_or("0")
        .to_string();

    let channel_info = ChannelInfo {
        title: channel_title.clone(),
        description: channel_info_value
            .get("snippet")
            .and_then(|s| s.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string(),
        thumbnail: format!("{}/channel_icon/{}", base.trim_end_matches('/'), channel_id),
        banner: if !banner.is_empty() {
            let encoded = urlencoding::encode(&banner);
            format!("{}/channel_icon/{}", base.trim_end_matches('/'), encoded)
        } else {
            "".to_string()
        },
        subscriber_count,
        video_count,
    };

    let mut videos: Vec<ChannelVideo> = Vec::new();
    let mut next_page_token: Option<String> = None;
    let mut total = 0;

    while total < count {
        let mut videos_url = format!(
            "https://www.googleapis.com/youtube/v3/search?part=snippet&channelId={}&maxResults=50&type=video&order=date&key={}",
            channel_id, apikey
        );
        if let Some(token) = &next_page_token {
            videos_url.push_str(&format!("&pageToken={}", token));
        }

        let videos_resp = match client.get(&videos_url).send().await {
            Ok(r) => r,
            Err(_) => break,
        };
        let videos_data: serde_json::Value = match videos_resp.json().await {
            Ok(d) => d,
            Err(_) => break,
        };

        let mut video_ids: Vec<String> = Vec::new();
        if let Some(items) = videos_data.get("items").and_then(|i| i.as_array()) {
            for item in items {
                if total >= count {
                    break;
                }
                if let Some(video_id) = item
                    .get("id")
                    .and_then(|id| id.get("videoId"))
                    .and_then(|v| v.as_str())
                {
                    video_ids.push(video_id.to_string());
                }
            }
        }

        let mut view_counts: HashMap<String, String> = HashMap::new();
        let mut durations: HashMap<String, String> = HashMap::new();
        if !video_ids.is_empty() {
            let ids = video_ids.join(",");
            let stats_url = format!(
                "https://www.googleapis.com/youtube/v3/videos?part=statistics,contentDetails&id={}&key={}",
                ids, apikey
            );
            if let Ok(stats_resp) = client.get(&stats_url).send().await {
                if let Ok(stats_data) = stats_resp.json::<serde_json::Value>().await {
                    if let Some(items) = stats_data.get("items").and_then(|i| i.as_array()) {
                        for item in items {
                            if let (Some(id), Some(stats)) = (
                                item.get("id").and_then(|i| i.as_str()),
                                item.get("statistics"),
                            ) {
                                let views = stats
                                    .get("viewCount")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("0")
                                    .to_string();
                                view_counts.insert(id.to_string(), views);
                            }
                            if let (Some(id), Some(details)) = (
                                item.get("id").and_then(|i| i.as_str()),
                                item.get("contentDetails"),
                            ) {
                                let iso = details
                                    .get("duration")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("");
                                durations.insert(id.to_string(), parse_iso_duration(iso));
                            }
                        }
                    }
                }
            }
        }

        if let Some(items) = videos_data.get("items").and_then(|i| i.as_array()) {
            for item in items {
                if total >= count {
                    break;
                }

                if let (Some(snippet), Some(video_id)) = (
                    item.get("snippet"),
                    item.get("id")
                        .and_then(|id| id.get("videoId"))
                        .and_then(|v| v.as_str()),
                ) {
                    let title = snippet
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();

                    let published_at = snippet
                        .get("publishedAt")
                        .and_then(|p| p.as_str())
                        .unwrap_or("")
                        .to_string();

                    let thumbnail =
                        format!("{}/thumbnail/{}", base.trim_end_matches('/'), video_id);

                    videos.push(ChannelVideo {
                        title,
                        author: channel_title.clone(),
                        video_id: video_id.to_string(),
                        thumbnail,
                        channel_thumbnail: format!(
                            "{}/channel_icon/{}",
                            base.trim_end_matches('/'),
                            channel_id
                        ),
                        views: view_counts
                            .get(video_id)
                            .cloned()
                            .unwrap_or_else(|| "0".to_string()),
                        published_at,
                        duration: durations
                            .get(video_id)
                            .cloned()
                            .unwrap_or_else(|| "0:00".to_string()),
                    });
                    total += 1;
                }
            }
        }

        next_page_token = videos_data
            .get("nextPageToken")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());

        if next_page_token.is_none() {
            break;
        }
    }

    (videos, channel_info)
}

#[utoipa::path(
    get,
    path = "/get_author_videos.php",
    params(
        ("author" = String, Query, description = "Channel username/search query"),
        ("count" = Option<i32>, Query, description = "Number of videos to return (default: 50)")
    ),
    responses(
        (status = 200, description = "Videos for the author", body = ChannelVideosResponse),
        (status = 400, description = "Missing author parameter")
    )
)]
pub async fn get_author_videos(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let author = match query_params.get("author") {
        Some(a) => a.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Author parameter is required"
            }));
        }
    };

    let count: i32 = query_params
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);

    let apikey = config.get_api_key_rotated();
    let client = Client::new();

    let search_url = format!(
        "https://www.googleapis.com/youtube/v3/search?part=snippet&q={}&type=channel&maxResults=1&key={}",
        urlencoding::encode(&author),
        apikey
    );

    let channel_id = match client.get(&search_url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(data) => data
                .get("items")
                .and_then(|i| i.as_array())
                .and_then(|arr| arr.get(0))
                .and_then(|item| item.get("id"))
                .and_then(|id| id.get("channelId"))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string()),
            Err(_) => None,
        },
        Err(_) => None,
    };

    let channel_id = match channel_id {
        Some(id) => id,
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Channel not found"
            }));
        }
    };

    get_author_videos_by_id_internal(&channel_id, count, config, &base).await
}

#[utoipa::path(
    get,
    path = "/get_author_videos_by_id.php",
    params(
        ("channel_id" = String, Query, description = "YouTube channel ID"),
        ("count" = Option<i32>, Query, description = "Number of videos to return (default: 50)")
    ),
    responses(
        (status = 200, description = "Videos for channel", body = ChannelVideosResponse),
        (status = 400, description = "Missing channel_id parameter")
    )
)]
pub async fn get_author_videos_by_id(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let channel_id = match query_params.get("channel_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Channel ID parameter is required"
            }));
        }
    };

    let count: i32 = query_params
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);

    get_author_videos_by_id_internal(&channel_id, count, config, &base).await
}

async fn get_author_videos_by_id_internal(
    channel_id: &str,
    count: i32,
    config: &crate::config::Config,
    base: &str,
) -> HttpResponse {
    let apikey = config.get_api_key_rotated();
    let (videos, channel_info) =
        fetch_channel_videos(channel_id, count, apikey, config, base).await;

    let response = ChannelVideosResponse {
        channel_info,
        videos,
    };

    HttpResponse::Ok().json(response)
}

#[utoipa::path(
    get,
    path = "/get_channel_thumbnail.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID")
    ),
    responses(
        (status = 200, description = "Channel thumbnail for video", body = serde_json::Value),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn get_channel_thumbnail_api(
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

    let config = &data.config;
    let apikey = config.get_api_key_rotated();
    if apikey.is_empty() {
        return HttpResponse::Ok().json(serde_json::json!({ "channel_thumbnail": "" }));
    }

    let client = Client::new();
    let video_url = format!(
        "https://www.googleapis.com/youtube/v3/videos?id={}&key={}&part=snippet",
        video_id, apikey
    );

    let channel_id = match client.get(&video_url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(data) => data
                .get("items")
                .and_then(|i| i.as_array())
                .and_then(|arr| arr.get(0))
                .and_then(|item| item.get("snippet"))
                .and_then(|s| s.get("channelId"))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string()),
            Err(_) => None,
        },
        Err(_) => None,
    };

    let channel_id = match channel_id {
        Some(id) => id,
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Видео не найдено"
            }));
        }
    };

    let thumb = fetch_channel_thumbnail(&channel_id, apikey)
        .await
        .unwrap_or_default();

    HttpResponse::Ok().json(serde_json::json!({
        "channel_thumbnail": thumb
    }))
}
