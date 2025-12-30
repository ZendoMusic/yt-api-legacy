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

#[derive(Serialize, ToSchema)]
pub struct CategoryItem {
    pub id: String,
    pub title: String,
}

#[derive(Serialize, ToSchema)]
pub struct PlaylistVideo {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
    pub views: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PlaylistInfo {
    pub title: String,
    pub description: String,
    pub thumbnail: String,
    pub channel_title: String,
    pub channel_thumbnail: String,
    pub video_count: i32,
}

#[derive(Serialize, ToSchema)]
pub struct PlaylistResponse {
    pub playlist_info: PlaylistInfo,
    pub videos: Vec<PlaylistVideo>,
}

async fn get_channel_thumbnail(channel_id: &str, api_key: &str, config: &crate::config::Config) -> String {
    // Если отключено прямое получение, возвращаем локальный прокси, чтобы иконки всегда были доступны.
    if !config.proxy.fetch_channel_thumbnails {
        return format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id);
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
                    format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id)
                }
                Err(_) => {
                    format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id)
                }
            }
        }
        Err(_) => {
            format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id)
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
                                let channel_id = video_info
                                    .get("channelId")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or(video_id);
                                let title = video_info.get("title")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("Unknown Title")
                                    .to_string();
                                
                                let author = video_info.get("channelTitle")
                                    .and_then(|a| a.as_str())
                                    .unwrap_or("Unknown Author")
                                    .to_string();
                                
                                let thumbnail = format!("{}/thumbnail/{}", config.server.mainurl.trim_end_matches('/'), video_id);
                                
                                let channel_thumbnail = format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id);
                                
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
                                        let channel_thumbnail = channel_id
                                            .as_ref()
                                            .map(|c| format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), c))
                                            .unwrap_or_else(|| format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), video_id.as_ref().unwrap()));
                                        
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
                                        
                                        let channel_thumbnail = channel_id
                                            .as_ref()
                                            .map(|c| format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), c))
                                            .unwrap_or_default();
                                        
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
    
    let encoded_query = urlencoding::encode(query);
    let url = format!(
        "https://clients1.google.com/complete/search?client=youtube&hl=en&ds=yt&q={}",
        encoded_query
    );
    
    match client.get(&url).send().await {
        Ok(response) => {
            match response.text().await {
                Ok(text) => {
                    let mut data = text.clone();
                    if data.starts_with("window.google.ac.h(") {
                        data = data.trim_start_matches("window.google.ac.h(").to_string();
                        if data.ends_with(')') {
                            data.pop();
                        }
                    }
                    if data.starts_with(")]}'") {
                        data = data.trim_start_matches(")]}'").to_string();
                    }
                    
                    match serde_json::from_str::<serde_json::Value>(&data) {
                        Ok(json_data) => {
                            let suggestions: Vec<String> = json_data
                                .get(1)
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .take(10)
                                        .filter_map(|item| {
                                            if let Some(suggestion_arr) = item.as_array() {
                                                suggestion_arr
                                                    .get(0)
                                                    .and_then(|v| v.as_str())
                                                    .map(|s| s.to_string())
                                            } else {
                                                item.as_str().map(|s| s.to_string())
                                            }
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            
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

#[utoipa::path(
    get,
    path = "/get-categories.php",
    params(
        ("region" = Option<String>, Query, description = "Region code (default: US)")
    ),
    responses(
        (status = 200, description = "List of categories", body = [CategoryItem]),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_categories(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let region = req.query_string()
        .split('&')
        .find_map(|pair| {
            let mut parts = pair.split('=');
            if parts.next() == Some("region") {
                parts.next().map(|v| v.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "US".to_string());
    
    let apikey = config.get_api_key_rotated();
    let url = format!(
        "https://www.googleapis.com/youtube/v3/videoCategories?part=snippet&regionCode={}&key={}",
        region,
        apikey
    );
    
    let client = Client::new();
    match client.get(&url).send().await {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json_data) => {
                    let mut categories = Vec::new();
                    if let Some(items) = json_data.get("items").and_then(|i| i.as_array()) {
                        for item in items {
                            if let (Some(id), Some(snippet)) = (
                                item.get("id").and_then(|i| i.as_str()),
                                item.get("snippet")
                            ) {
                                let title = snippet
                                    .get("title")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                
                                categories.push(CategoryItem {
                                    id: id.to_string(),
                                    title,
                                });
                            }
                        }
                    }
                    
                    HttpResponse::Ok().json(categories)
                }
                Err(e) => {
                    crate::log::info!("Error parsing categories response: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to parse categories response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling categories API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call categories API"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/get-categories_videos.php",
    params(
        ("count" = Option<i32>, Query, description = "Number of videos to return (default: 50)"),
        ("categoryId" = Option<String>, Query, description = "YouTube category ID")
    ),
    responses(
        (status = 200, description = "Videos from a category", body = [TopVideo]),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_categories_videos(
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
    
    let count: i32 = query_params.get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);
    
    let category_id = query_params.get("categoryId").cloned();
    let apikey = config.get_api_key_rotated();
    
    let mut url = format!(
        "https://www.googleapis.com/youtube/v3/videos?part=snippet&chart=mostPopular&maxResults={}&key={}",
        count,
        apikey
    );
    
    if let Some(cat) = category_id {
        url.push_str(&format!("&videoCategoryId={}", cat));
    }
    
    let client = Client::new();
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
                                
                                let channel_thumbnail = video_info
                                    .get("channelId")
                                    .and_then(|c| c.as_str())
                                    .map(|c| format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), c))
                                    .unwrap_or_else(|| format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), video_id));
                                
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
                    crate::log::info!("Error parsing category videos response: {}", e);
                    HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to parse response"
                    }))
                }
            }
        }
        Err(e) => {
            crate::log::info!("Error calling category videos API: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to call YouTube API"
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/playlist",
    responses(
        (status = 400, description = "Playlist ID missing")
    )
)]
pub async fn playlist_root() -> impl Responder {
    HttpResponse::BadRequest().json(serde_json::json!({
        "error": "Playlist ID is required. Use /playlist/PLAYLIST_ID"
    }))
}

#[utoipa::path(
    get,
    path = "/playlist/{playlist_id}",
    params(
        ("playlist_id" = String, Path, description = "YouTube playlist ID"),
        ("count" = Option<i32>, Query, description = "Number of items to return (default: 50)")
    ),
    responses(
        (status = 200, description = "Playlist metadata and videos", body = PlaylistResponse),
        (status = 400, description = "Playlist ID missing"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_playlist_videos(
    path: web::Path<String>,
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let playlist_id = path.into_inner();
    if playlist_id.is_empty() {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Playlist ID parameter is required"
        }));
    }
    
    let config = &data.config;
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }
    let count: i32 = query_params.get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);
    
    let apikey = config.get_api_key_rotated();
    let client = Client::new();
    
    let playlist_url = format!(
        "https://www.googleapis.com/youtube/v3/playlists?part=snippet,contentDetails&id={}&key={}",
        playlist_id, apikey
    );
    
    let playlist_resp = match client.get(&playlist_url).send().await {
        Ok(r) => r,
        Err(e) => {
            crate::log::info!("Error fetching playlist info: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch playlist"
            }));
        }
    };
    
    let playlist_data: serde_json::Value = match playlist_resp.json().await {
        Ok(d) => d,
        Err(e) => {
            crate::log::info!("Error parsing playlist info: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to parse playlist"
            }));
        }
    };
    
    let playlist_info = match playlist_data.get("items").and_then(|i| i.as_array()).and_then(|arr| arr.get(0)) {
        Some(info) => info,
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Playlist not found"
            }));
        }
    };
    
    let channel_id = playlist_info
        .get("snippet")
        .and_then(|s| s.get("channelId"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    
    let channel_resp = client.get(format!(
        "https://www.googleapis.com/youtube/v3/channels?part=snippet,statistics&id={}&key={}",
        channel_id, apikey
    )).send().await;
    
    let channel_data: serde_json::Value = match channel_resp {
        Ok(r) => match r.json().await {
            Ok(d) => d,
            Err(_) => serde_json::json!({}),
        },
        Err(_) => serde_json::json!({}),
    };
    
    let channel_info = channel_data.get("items")
        .and_then(|i| i.as_array())
        .and_then(|arr| arr.get(0));
    
    let mut videos: Vec<PlaylistVideo> = Vec::new();
    let mut next_page_token: Option<String> = None;
    let mut total = 0;
    
    while total < count {
        let mut playlist_items_url = format!(
            "https://www.googleapis.com/youtube/v3/playlistItems?part=snippet,contentDetails&playlistId={}&maxResults=50&key={}",
            playlist_id, apikey
        );
        if let Some(token) = &next_page_token {
            playlist_items_url.push_str(&format!("&pageToken={}", token));
        }
        
        let items_resp = match client.get(&playlist_items_url).send().await {
            Ok(r) => r,
            Err(e) => {
                crate::log::info!("Error fetching playlist items: {}", e);
                break;
            }
        };
        
        let items_data: serde_json::Value = match items_resp.json().await {
            Ok(d) => d,
            Err(e) => {
                crate::log::info!("Error parsing playlist items: {}", e);
                break;
            }
        };
        
        if let Some(items) = items_data.get("items").and_then(|i| i.as_array()) {
            for item in items {
                if total >= count {
                    break;
                }
                
                if let (Some(snippet), Some(content_details)) = (
                    item.get("snippet"),
                    item.get("contentDetails")
                ) {
                    if let Some(video_id) = content_details
                        .get("videoId")
                        .and_then(|v| v.as_str()) {
                        
                        let title = snippet
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        
                        let author = channel_info
                            .and_then(|c| c.get("snippet"))
                            .and_then(|s| s.get("title"))
                            .and_then(|t| t.as_str())
                            .unwrap_or_else(|| snippet.get("channelTitle").and_then(|t| t.as_str()).unwrap_or(""))
                            .to_string();
                        
                        let thumbnail = format!("{}/thumbnail/{}", config.server.mainurl.trim_end_matches('/'), video_id);
                        
                        let channel_thumbnail = channel_info
                            .and_then(|c| c.get("snippet"))
                            .and_then(|s| s.get("thumbnails"))
                            .and_then(|t| t.get("high"))
                            .and_then(|h| h.get("url"))
                            .and_then(|u| u.as_str())
                            .map(|u| u.to_string())
                            .unwrap_or_else(|| format!("{}/channel_icon/{}", config.server.mainurl.trim_end_matches('/'), channel_id));
                        
                        videos.push(PlaylistVideo {
                            title,
                            author,
                            video_id: video_id.to_string(),
                            thumbnail,
                            channel_thumbnail,
                            views: None,
                            published_at: snippet
                                .get("publishedAt")
                                .and_then(|p| p.as_str())
                                .map(|s| s.to_string()),
                        });
                        total += 1;
                    }
                }
            }
        }
        
        next_page_token = items_data
            .get("nextPageToken")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());
        
        if next_page_token.is_none() {
            break;
        }
    }
    
    let first_video_id = videos.first().map(|v| v.video_id.clone()).unwrap_or_default();
    
    let playlist_info_resp = PlaylistInfo {
        title: playlist_info.get("snippet")
            .and_then(|s| s.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        description: playlist_info.get("snippet")
            .and_then(|s| s.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string(),
        thumbnail: if !first_video_id.is_empty() {
            format!("{}/thumbnail/{}", config.server.mainurl.trim_end_matches('/'), first_video_id)
        } else {
            "".to_string()
        },
        channel_title: channel_info
            .and_then(|c| c.get("snippet"))
            .and_then(|s| s.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        channel_thumbnail: channel_info
            .and_then(|c| c.get("snippet"))
            .and_then(|s| s.get("thumbnails"))
            .and_then(|t| t.get("high"))
            .and_then(|h| h.get("url"))
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string(),
        video_count: playlist_info.get("contentDetails")
            .and_then(|c| c.get("itemCount"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
    };
    
    let response = PlaylistResponse {
        playlist_info: playlist_info_resp,
        videos,
    };
    
    HttpResponse::Ok().json(response)
}
