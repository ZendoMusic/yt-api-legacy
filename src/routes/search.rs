use actix_web::{web, HttpResponse, Responder, HttpRequest};
use serde::Serialize;
use utoipa::ToSchema;
use reqwest::Client;
use std::collections::HashMap;

#[derive(Serialize, ToSchema)]
pub struct TopVideo {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
}

#[derive(Serialize, ToSchema)]
pub struct SearchResult {
    pub title: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist_id: Option<String>,
    pub thumbnail: String,
    pub channel_thumbnail: String,
}

#[derive(Serialize, ToSchema)]
pub struct SearchSuggestions {
    pub query: String,
    pub suggestions: Vec<String>,
}

async fn get_channel_thumbnail(channel_id: &str, api_key: &str, config: &crate::config::Config) -> String {
    if !config.proxy.fetch_channel_thumbnails {
        return "".to_string();
    }
    
    let client = Client::new();
    let url = format!(
        "https://www.googleapis.com/youtube/v3/channels?id={}&key={}&part=snippet",
        channel_id, api_key
    );
    
    match client.get(&url).send().await {
        Ok(response) => {
            match response.json::<serde_json::Value>().await {
                Ok(data) => {
                    if let Some(items) = data.get("items").and_then(|i| i.as_array()) {
                        if !items.is_empty() {
                            if let Some(thumbnail_url) = items[0]
                                .get("snippet")
                                .and_then(|s| s.get("thumbnails"))
                                .and_then(|t| t.get("default"))
                                .and_then(|d| d.get("url"))
                                .and_then(|u| u.as_str()) {
                                return thumbnail_url.replace("https://yt3.ggpht.com", "https://yt3.googleusercontent.com");
                            }
                        }
                    }
                    "https://yt3.googleusercontent.com/a/default-user=s88-c-k-c0x00ffffff-no-rj".to_string()
                }
                Err(_) => {
                    "https://yt3.googleusercontent.com/a/default-user=s88-c-k-c0x00ffffff-no-rj".to_string()
                }
            }
        }
        Err(_) => {
            "https://yt3.googleusercontent.com/a/default-user=s88-c-k-c0x00ffffff-no-rj".to_string()
        }
    }
}

#[utoipa::path(
    get,
    path = "/get_top_videos.php",
    params(
        ("count" = Option<i32>, Query, description = "Number of videos to return (default: 50)")
    ),
    responses(
        (status = 200, description = "List of top videos", body = [TopVideo]),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_top_videos(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    
    let count: i32 = req.query_string()
        .split('&')
        .find_map(|pair| {
            let mut parts = pair.split('=');
            if parts.next() == Some("count") {
                parts.next().and_then(|v| v.parse().ok())
            } else {
                None
            }
        })
        .unwrap_or(config.video.default_count as i32);
    
    let count = count.min(50).max(1);
    
    let apikey = config.get_api_key_rotated();
    
    let client = Client::new();
    
    let url = format!(
        "https://www.googleapis.com/youtube/v3/videos?part=snippet&chart=mostPopular&maxResults={}&key={}",
        count,
        apikey
    );
    
    match client.get(&url).send().await {
        Ok(response) => {
            match response.json::<serde_json::Value>().await {
                Ok(json_data) => {
                    let mut top_videos: Vec<TopVideo> = Vec::new();
                    
                    if let Some(items) = json_data.get("items").and_then(|i| i.as_array()) {
                        for video in items {
                            if let (Some(video_info), Some(video_id)) = (
                                video.get("snippet"),
                                video.get("id").and_then(|id| id.as_str())
                            ) {
                                let title = video_info.get("title")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("Unknown Title")
                                    .to_string();
                                
                                let author = video_info.get("channelTitle")
                                    .and_then(|a| a.as_str())
                                    .unwrap_or("Unknown Author")
                                    .to_string();
                                
                                let thumbnail = format!("{}/thumbnail/{}", config.server.mainurl.trim_end_matches('/'), video_id);
                                
                                let channel_thumbnail = "".to_string();
                                
                                top_videos.push(TopVideo {
                                    title,
                                    author,
                                    video_id: video_id.to_string(),
                                    thumbnail,
                                    channel_thumbnail,
                                });
                            }
                        }
                    }
                    
                    HttpResponse::Ok().json(top_videos)
                }
                Err(e) => {
                    crate::log::info!("Error parsing YouTube API response: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to parse YouTube API response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling YouTube API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call YouTube API"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/get_search_videos.php",
    params(
        ("query" = String, Query, description = "Search query"),
        ("count" = Option<i32>, Query, description = "Number of results to return (default: 50)"),
        ("type" = Option<String>, Query, description = "Type of search results (video, channel, playlist) (default: video)")
    ),
    responses(
        (status = 200, description = "List of search results", body = [SearchResult]),
        (status = 400, description = "Missing query parameter"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_search_videos(
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
    
    let query = match query_params.get("query") {
        Some(q) => q.clone(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "query parameter not specified"
            }));
        }
    };
    
    let count: i32 = query_params.get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);
    
    let search_type = query_params.get("type")
        .map(|t| t.as_str())
        .unwrap_or("video");
    
    let valid_types = ["video", "channel", "playlist"];
    if !valid_types.contains(&search_type) {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": format!("Invalid type parameter. Must be one of: {}", valid_types.join(", "))
        }));
    }
    
    let apikey = config.get_api_key_rotated();
    let client = Client::new();
    
    let url = format!(
        "https://www.googleapis.com/youtube/v3/search?part=snippet&q={}&maxResults={}&type={}&key={}",
        query,
        count,
        search_type,
        apikey
    );
    
    match client.get(&url).send().await {
        Ok(response) => {
            match response.json::<serde_json::Value>().await {
                Ok(json_data) => {
                    let mut search_results: Vec<SearchResult> = Vec::new();
                    
                    if let Some(items) = json_data.get("items").and_then(|i| i.as_array()) {
                        for (_index, item) in items.iter().enumerate() {
                            if let Some(item_id) = item.get("id") {
                                let item_info = match item.get("snippet") {
                                    Some(info) => info,
                                    None => {
                                        continue;
                                    }
                                };
                                
                                let title = item_info.get("title")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("Unknown Title")
                                    .to_string();
                                
                                let author = item_info.get("channelTitle")
                                    .and_then(|a| a.as_str())
                                    .unwrap_or("Unknown Author")
                                    .to_string();
                                
                                let channel_id = item_info.get("channelId")
                                    .and_then(|id| id.as_str())
                                    .map(|id| id.to_string());
                                
                                let result = match search_type {
                                    "video" => {
                                        let video_id = item_id.get("videoId")
                                            .and_then(|id| id.as_str())
                                            .map(|id| id.to_string());
                                        
                                        if video_id.is_none() {
                                            continue;
                                        }
                                        
                                        let thumbnail = format!("{}/thumbnail/{}", config.server.mainurl.trim_end_matches('/'), video_id.as_ref().unwrap());
                                        let channel_thumbnail = if let Some(channel_id_str) = &channel_id {
                                            get_channel_thumbnail(channel_id_str, &apikey, config).await
                                        } else {
                                            "".to_string()
                                        };
                                        
                                        SearchResult {
                                            title,
                                            author,
                                            video_id,
                                            channel_id,
                                            playlist_id: None,
                                            thumbnail,
                                            channel_thumbnail,
                                        }
                                    },
                                    "channel" => {
                                        let channel_id = item_id.get("channelId")
                                            .and_then(|id| id.as_str())
                                            .map(|id| id.to_string());
                                        
                                        if channel_id.is_none() {
                                            continue;
                                        }
                                        
                                        let thumbnail = item_info.get("thumbnails")
                                            .and_then(|thumbs| thumbs.get("high"))
                                            .and_then(|high| high.get("url"))
                                            .and_then(|url| url.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        
                                        let channel_thumbnail = format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id.as_ref().unwrap());
                                        
                                        SearchResult {
                                            title,
                                            author,
                                            video_id: None,
                                            channel_id,
                                            playlist_id: None,
                                            thumbnail,
                                            channel_thumbnail,
                                        }
                                    },
                                    "playlist" => {
                                        let playlist_id = item_id.get("playlistId")
                                            .and_then(|id| id.as_str())
                                            .map(|id| id.to_string());
                                        
                                        if playlist_id.is_none() {
                                            continue;
                                        }
                                        
                                        let mut video_id = String::new();
                                        if let Some(thumbnail_url) = item_info.get("thumbnails")
                                            .and_then(|thumbs| thumbs.get("high"))
                                            .and_then(|high| high.get("url"))
                                            .and_then(|url| url.as_str()) {
                                            if thumbnail_url.contains("i.ytimg.com/vi/") {
                                                if let Some(start) = thumbnail_url.find("i.ytimg.com/vi/") {
                                                    let start_pos = start + 16;
                                                    if let Some(end_pos) = thumbnail_url[start_pos..].find('/') {
                                                        video_id = thumbnail_url[start_pos..start_pos + end_pos].to_string();
                                                    }
                                                }
                                            }
                                        }
                                        
                                        let thumbnail = if !video_id.is_empty() {
                                            format!("{}/thumbnail/{}", config.server.mainurl.trim_end_matches('/'), video_id)
                                        } else {
                                            item_info.get("thumbnails")
                                                .and_then(|thumbs| thumbs.get("high"))
                                                .and_then(|high| high.get("url"))
                                                .and_then(|url| url.as_str())
                                                .unwrap_or("")
                                                .to_string()
                                        };
                                        
                                        let channel_thumbnail = if let Some(channel_id_str) = &channel_id {
                                            get_channel_thumbnail(channel_id_str, &apikey, config).await
                                        } else {
                                            "".to_string()
                                        };
                                        
                                        SearchResult {
                                            title,
                                            author,
                                            video_id: None,
                                            channel_id,
                                            playlist_id,
                                            thumbnail,
                                            channel_thumbnail,
                                        }
                                    },
                                    _ => continue,
                                };
                                
                                search_results.push(result);
                            }
                        }
                    }
                    
                    HttpResponse::Ok().json(search_results)
                }
                Err(e) => {
                    crate::log::info!("Error parsing YouTube API response: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to parse YouTube API response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling YouTube API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call YouTube API"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/get_search_suggestions.php",
    params(
        ("query" = String, Query, description = "Search query for suggestions")
    ),
    responses(
        (status = 200, description = "Search suggestions", body = SearchSuggestions),
        (status = 400, description = "Missing query parameter"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_search_suggestions(
    req: HttpRequest,
    _data: web::Data<crate::AppState>,
) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    
    let query = match query_params.get("query") {
        Some(q) => q,
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Query parameter is required"
            }));
        }
    };
    
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()
        .unwrap();
    
    let url = format!(
        "https://clients1.google.com/complete/search?client=youtube&hl=en&ds=yt&q={}",
        query
    );
    
    match client.get(&url).send().await {
        Ok(response) => {
            match response.text().await {
                Ok(text) => {
                    let mut data = text.clone();
                    if data.starts_with("window.google.ac.h(") {
                        data = data[19..].to_string();
                        if data.ends_with(')') {
                            data.pop();
                        }
                    }
                    
                    match serde_json::from_str::<serde_json::Value>(&data) {
                        Ok(json_data) => {
                            let suggestions: Vec<String> = if let Some(arr) = json_data.get(1).and_then(|v| v.as_array()) {
                                arr.iter()
                                    .take(10)
                                    .filter_map(|item| {
                                        if let Some(suggestion_arr) = item.as_array() {
                                            if !suggestion_arr.is_empty() {
                                                suggestion_arr[0].as_str().map(|s| s.to_string())
                                            } else {
                                                None
                                            }
                                        } else {
                                            item.as_str().map(|s| s.to_string())
                                        }
                                    })
                                    .collect()
                            } else {
                                Vec::new()
                            };
                            
                            let result = SearchSuggestions {
                                query: query.clone(),
                                suggestions,
                            };
                            
                            HttpResponse::Ok().json(result)
                        }
                        Err(e) => {
                            crate::log::info!("Error parsing suggestions JSON: {} - Data: {}", e, data);
                            HttpResponse::InternalServerError().json(serde_json::json!({
                                "error": "Failed to parse suggestions response"
                            }))
                        }
                    }
                }
                Err(e) => {
                    crate::log::info!("Error reading suggestions response: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to read suggestions response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling suggestions API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call suggestions API"
            }))
        }
    }
}