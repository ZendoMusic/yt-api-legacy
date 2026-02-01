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

fn parse_number(text: &str) -> String {
    let lower_text = text.trim().to_lowercase();
    let mut multiplier = 1.0;
    let clean_text = if lower_text.contains('k') {
        multiplier = 1000.0;
        lower_text.replace('k', "")
    } else if lower_text.contains('m') {
        multiplier = 1000000.0;
        lower_text.replace('m', "")
    } else if lower_text.contains('b') {
        multiplier = 1000000000.0;
        lower_text.replace('b', "")
    } else {
        lower_text
    };
    
    // Extract digits and decimal points
    let num_str: String = clean_text.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    
    match num_str.parse::<f64>() {
        Ok(num) => ((num * multiplier) as u64).to_string(),
        Err(_) => "0".to_string(),
    }
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

    // Use InnerTube API key from config
    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };

    let client = Client::new();

    // Resolve handle to channel ID using InnerTube API
    let channel_id = resolve_handle_to_channel_id(&author, &client, &innertube_key, &base).await;

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
    // Use InnerTube API key from config
    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };

    let (videos, channel_info) = fetch_channel_videos_inner_tube(channel_id, count, &innertube_key, base).await;

    let response = ChannelVideosResponse {
        channel_info,
        videos,
    };

    HttpResponse::Ok().json(response)
}

async fn resolve_handle_to_channel_id(handle: &str, client: &Client, innertube_key: &str, _base: &str) -> Option<String> {
    let clean_handle = handle.trim().trim_start_matches('@');
    let url = format!("https://www.youtube.com/youtubei/v1/navigation/resolve_url?key={}&prettyPrint=false", innertube_key);
    
    let context = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20260220.00.00",
            "hl": "en",
            "gl": "US"
        }
    });
    
    let payload = serde_json::json!({
        "context": context,
        "url": format!("https://www.youtube.com/@{}", clean_handle),
        "request": {}
    });
    
    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(data) => {
                data.get("endpoint")
                    .and_then(|endpoint| endpoint.get("browseEndpoint"))
                    .and_then(|browse_endpoint| browse_endpoint.get("browseId"))
                    .and_then(|browse_id| browse_id.as_str())
                    .map(|s| s.to_string())
            },
            Err(_) => None,
        },
        Err(_) => None,
    }
}

async fn fetch_channel_videos_inner_tube(
    channel_id: &str,
    count: i32,
    innertube_key: &str,
    base: &str,
) -> (Vec<ChannelVideo>, ChannelInfo) {
    let client = Client::new();
    
    let url = format!("https://www.youtube.com/youtubei/v1/browse?key={}&prettyPrint=false", innertube_key);
    
    let context = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20260220.00.00",
            "hl": "en",
            "gl": "US"
        }
    });
    
    let payload = serde_json::json!({
        "context": context,
        "browseId": channel_id,
    });
    
    let response = match client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(_) => {
            return (Vec::new(), ChannelInfo {
                title: "Unknown".to_string(),
                description: "".to_string(),
                thumbnail: "".to_string(),
                banner: "".to_string(),
                subscriber_count: "0".to_string(),
                video_count: "0".to_string(),
            });
        }
    };
    
    let data: serde_json::Value = match response.json().await {
        Ok(json) => json,
        Err(_) => {
            return (Vec::new(), ChannelInfo {
                title: "Unknown".to_string(),
                description: "".to_string(),
                thumbnail: "".to_string(),
                banner: "".to_string(),
                subscriber_count: "0".to_string(),
                video_count: "0".to_string(),
            });
        }
    };
    
    let channel_info = extract_channel_info(&data, base, channel_id).await;
    
    // Find the Videos tab
    let tabs_option = data
        .get("contents")
        .and_then(|c| c.get("twoColumnBrowseResultsRenderer"))
        .and_then(|r| r.get("tabs"))
        .and_then(|t| t.as_array());
    
    let tabs_array: &Vec<serde_json::Value> = match tabs_option {
        Some(arr) => arr,
        None => &Vec::new(),
    };
    
    let mut videos_content = None;
    for tab in tabs_array {
        if let Some(tr) = tab.get("tabRenderer") {
            if let Some(title) = tr.get("title").and_then(|t| t.as_str()) {
                if title == "Videos" {
                    videos_content = tr.get("content");
                    break;
                }
            }
        }
    }
    
    // If no "Videos" tab found, try to find any selected tab
    if videos_content.is_none() {
        for tab in tabs_array {
            if let Some(tr) = tab.get("tabRenderer") {
                if tr.get("selected").and_then(|s| s.as_bool()).unwrap_or(false) {
                    videos_content = tr.get("content");
                    break;
                }
            }
        }
    }
    
    let mut videos = Vec::new();
    if let Some(content) = videos_content {
        videos = collect_videos_from_content(content, &channel_info, base, &client, innertube_key).await;
    }
    
    // Limit the number of videos
    videos.truncate(count as usize);
    
    (videos, channel_info)
}

async fn extract_channel_info(data: &serde_json::Value, base: &str, channel_id: &str) -> ChannelInfo {
    let metadata = data
        .get("metadata")
        .and_then(|m| m.get("channelMetadataRenderer"))
        .unwrap_or(&serde_json::Value::Null);
    
    let title = metadata
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("No title")
        .to_string();
    
    let description = metadata
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    
    let external_id = metadata
        .get("externalId")
        .and_then(|id| id.as_str())
        .unwrap_or(channel_id);
    
    // Extract banner URL
    let banner_url = data
        .get("header")
        .and_then(|h| h.get("pageHeaderRenderer"))
        .and_then(|ph| ph.get("content"))
        .and_then(|c| c.get("pageHeaderViewModel"))
        .and_then(|phvm| phvm.get("banner"))
        .and_then(|b| b.get("imageBannerViewModel"))
        .and_then(|ibvm| ibvm.get("image"))
        .and_then(|img| img.get("sources"))
        .and_then(|sources| sources.as_array())
        .and_then(|arr| arr.last())
        .and_then(|last_source| last_source.get("url"))
        .and_then(|url| url.as_str())
        .unwrap_or("");
    
    let banner_escaped = if !banner_url.is_empty() {
        urlencoding::encode(banner_url).to_string()
    } else {
        "".to_string()
    };
    
    let channel_icon = format!("{}/channel_icon/{}", base.trim_end_matches('/'), external_id);
    let banner = if !banner_escaped.is_empty() {
        format!("{}/channel_icon/{}", base.trim_end_matches('/'), banner_escaped)
    } else {
        "".to_string()
    };
    
    // Extract subscriber and video counts
    let mut subscriber_count = "0".to_string();
    let mut video_count = "0".to_string();
    
    if let Some(header) = data
        .get("header")
        .and_then(|h| h.get("pageHeaderRenderer"))
        .and_then(|ph| ph.get("content"))
        .and_then(|c| c.get("pageHeaderViewModel"))
        .and_then(|phvm| phvm.get("metadata"))
        .and_then(|m| m.get("contentMetadataViewModel"))
        .and_then(|cmvm| cmvm.get("metadataRows"))
        .and_then(|mr| mr.as_array())
    {
        if header.len() > 1 {
            if let Some(row) = header.get(1) {
                if let Some(parts) = row.get("metadataParts").and_then(|mp| mp.as_array()) {
                    if let Some(sub_part) = parts.first() {
                        if let Some(content) = sub_part
                            .get("text")
                            .and_then(|t| t.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            subscriber_count = parse_number(content);
                        }
                    }
                    
                    if parts.len() > 1 {
                        if let Some(video_part) = parts.get(1) {
                            if let Some(content) = video_part
                                .get("text")
                                .and_then(|t| t.get("content"))
                                .and_then(|c| c.as_str())
                            {
                                video_count = parse_number(content);
                            }
                        }
                    }
                }
            }
        }
    }
    
    ChannelInfo {
        title,
        description,
        thumbnail: channel_icon,
        banner,
        subscriber_count,
        video_count,
    }
}

async fn collect_videos_from_content(
    content: &serde_json::Value,
    channel_info: &ChannelInfo,
    base: &str,
    _client: &Client,
    _innertube_key: &str,
) -> Vec<ChannelVideo> {
    let mut videos = Vec::new();
    
    // Process sectionListRenderer
    if let Some(section_list) = content.get("sectionListRenderer") {
        if let Some(sections) = section_list.get("contents").and_then(|c| c.as_array()) {
            for section in sections {
                if let Some(item_section) = section.get("itemSectionRenderer") {
                    if let Some(contents) = item_section.get("contents").and_then(|c| c.as_array()) {
                        for item in contents {
                            if let Some(grid_video) = item.get("gridVideoRenderer") {
                                if let Some(video) = process_grid_video_renderer(grid_video, channel_info, base).await {
                                    videos.push(video);
                                }
                            } else if let Some(shelf_renderer) = item.get("shelfRenderer") {
                                if let Some(horizontal_list) = shelf_renderer
                                    .get("content")
                                    .and_then(|c| c.get("horizontalListRenderer"))
                                    .and_then(|hlr| hlr.get("items"))
                                    .and_then(|i| i.as_array())
                                {
                                    for horiz_item in horizontal_list {
                                        if let Some(grid_video) = horiz_item.get("gridVideoRenderer") {
                                            if let Some(video) = process_grid_video_renderer(grid_video, channel_info, base).await {
                                                videos.push(video);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Process richGridRenderer
    if let Some(rich_grid) = content.get("richGridRenderer") {
        if let Some(contents) = rich_grid.get("contents").and_then(|c| c.as_array()) {
            for item in contents {
                if let Some(grid_video) = item.get("gridVideoRenderer") {
                    if let Some(video) = process_grid_video_renderer(grid_video, channel_info, base).await {
                        videos.push(video);
                    }
                }
            }
        }
    }
    
    videos
}

async fn process_grid_video_renderer(
    vr: &serde_json::Value,
    channel_info: &ChannelInfo,
    base: &str,
) -> Option<ChannelVideo> {
    let video_id = vr.get("videoId").and_then(|id| id.as_str())?;
    let title_obj = vr.get("title")?;
    
    let title = if let Some(simple_text) = title_obj.get("simpleText").and_then(|t| t.as_str()) {
        simple_text.to_string()
    } else if let Some(runs) = title_obj.get("runs").and_then(|r| r.as_array()) {
        if let Some(first_run) = runs.first() {
            first_run.get("text").and_then(|t| t.as_str()).unwrap_or("No title").to_string()
        } else {
            "No title".to_string()
        }
    } else {
        "No title".to_string()
    };
    
    let views_raw = vr
        .get("viewCountText")
        .and_then(|vct| vct.get("simpleText"))
        .or_else(|| vr.get("shortViewCountText").and_then(|svct| svct.get("simpleText")))
        .and_then(|st| st.as_str())
        .unwrap_or("0");
    
    let views = parse_number(views_raw);
    
    let duration = vr
        .get("lengthText")
        .and_then(|lt| lt.get("simpleText"))
        .or_else(|| {
            vr.get("thumbnailOverlays")
                .and_then(|to| to.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first_overlay| first_overlay.get("thumbnailOverlayTimeStatusRenderer"))
                .and_then(|totsr| totsr.get("text"))
                .and_then(|text| text.get("simpleText"))
        })
        .and_then(|st| st.as_str())
        .unwrap_or("0:00")
        .to_string();
    
    // Extract published time text like "2 weeks ago" from the JSON
    let published_at_raw = vr
        .get("publishedTimeText")
        .and_then(|ptt| ptt.get("simpleText"))
        .and_then(|st| st.as_str())
        .unwrap_or("");
    
    // Convert human-readable time to a standard format or keep as is
    let published_at = if !published_at_raw.is_empty() {
        published_at_raw.to_string()
    } else {
        "1970-01-01T00:00:00Z".to_string()
    };
    
    Some(ChannelVideo {
        title,
        author: channel_info.title.clone(),
        video_id: video_id.to_string(),
        thumbnail: format!("{}/thumbnail/{}", base.trim_end_matches('/'), video_id),
        channel_thumbnail: channel_info.thumbnail.clone(),
        views,
        published_at,
        duration,
    })
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
    _data: web::Data<crate::AppState>,
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

    // Return the URL for the channel icon based on the video ID
    // The actual thumbnail will be fetched by the channel_icon endpoint
    let channel_thumbnail_url = format!("{}/channel_icon/{}", 
        base_url(&req, &_data.config).trim_end_matches('/'), 
        video_id
    );

    HttpResponse::Ok().json(serde_json::json!({
        "channel_thumbnail": channel_thumbnail_url
    }))
}