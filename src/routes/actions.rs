use actix_web::{http::StatusCode as ActixStatusCode, web, HttpResponse, Responder};
use lazy_static::lazy_static;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use crate::routes::auth::AuthConfig;
use crate::routes::oauth::refresh_access_token;

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/120 Safari/537.36";

lazy_static! {
    static ref CHANNEL_PATH_REGEX: Regex =
        Regex::new(r"/channel/(UC[0-9A-Za-z_-]{20,})").expect("valid regex");
    static ref EXTERNAL_ID_REGEX: Regex =
        Regex::new(r#""externalId"\s*:\s*"(?P<id>UC[0-9A-Za-z_-]{20,})""#).expect("valid regex");
    static ref CHANNEL_ID_REGEX: Regex =
        Regex::new(r#"channelId":"(UC[0-9A-Za-z_-]{20,})"#).expect("valid regex");
}

#[derive(Deserialize, ToSchema)]
pub struct YoutubeSubscriptionRequest {
    pub channel: String,
    pub token: String,
}

#[derive(Deserialize, ToSchema)]
pub struct YoutubeRateRequest {
    pub video_id: String,
    pub rating: String,
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct YoutubeActionResponse {
    pub status: String,
    pub action: String,
    pub channel_id: Option<String>,
    pub video_id: Option<String>,
    pub message: String,
}

#[derive(Deserialize, ToSchema)]
pub struct RatingCheckRequest {
    pub video_id: String,
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct RatingCheckResponse {
    pub status: String,
    pub video_id: String,
    pub rating: String,
}

#[derive(Deserialize, ToSchema)]
pub struct SubscriptionCheckRequest {
    pub channel: String,
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct SubscriptionCheckResponse {
    pub status: String,
    pub channel_id: String,
    pub subscribed: bool,
}

fn error_json(status: ActixStatusCode, message: impl ToString) -> HttpResponse {
    HttpResponse::build(status).json(json!({ "error": message.to_string() }))
}

async fn obtain_access_token(
    refresh_token: &str,
    auth_config: &AuthConfig,
) -> Result<String, HttpResponse> {
    let trimmed = refresh_token.trim();
    if trimmed.is_empty() {
        return Err(error_json(
            ActixStatusCode::BAD_REQUEST,
            "Missing refresh_token",
        ));
    }

    match refresh_access_token(trimmed, auth_config).await {
        Ok(token) => Ok(token),
        Err(err) => Err(error_json(ActixStatusCode::UNAUTHORIZED, err)),
    }
}

fn build_channel_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else if trimmed.starts_with('@') {
        format!("https://www.youtube.com/{}", trimmed)
    } else {
        format!("https://www.youtube.com/{}", trimmed)
    }
}

async fn resolve_channel_id(input: &str, client: &Client) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Channel identifier cannot be empty".to_string());
    }

    if trimmed.starts_with("UC") && trimmed.len() >= 24 {
        return Ok(trimmed.to_string());
    }

    let target_url = build_channel_url(trimmed);
    let res = client
        .get(&target_url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!(
            "Failed to fetch channel page: {}",
            res.status().as_u16()
        ));
    }

    let final_url = res.url().to_string();
    if let Some(caps) = CHANNEL_PATH_REGEX.captures(&final_url) {
        if let Some(id) = caps.get(1) {
            return Ok(id.as_str().to_string());
        }
    }

    let body = res.text().await.map_err(|e| e.to_string())?;
    if let Some(caps) = EXTERNAL_ID_REGEX.captures(&body) {
        if let Some(id) = caps.name("id") {
            return Ok(id.as_str().to_string());
        }
    }

    if let Some(caps) = CHANNEL_ID_REGEX.captures(&body) {
        if let Some(id) = caps.get(1) {
            return Ok(id.as_str().to_string());
        }
    }

    Err("Unable to resolve channel id".to_string())
}

/// YouTube Data API v3: subscriptions.insert (как в new_endpoints/subscribe_innertube.py).
async fn subscribe_channel_api(
    client: &Client,
    channel_id: &str,
    access_token: &str,
) -> Result<serde_json::Value, String> {
    let payload = json!({
        "snippet": {
            "resourceId": {
                "kind": "youtube#channel",
                "channelId": channel_id
            }
        }
    });
    let resp = client
        .post("https://www.googleapis.com/youtube/v3/subscriptions?part=snippet")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let body = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!("YouTube API error {}: {}", status.as_u16(), text))
    }
}

/// YouTube Data API v3: subscriptions.list (mine=true, forChannelId) — как в new_endpoints.
async fn find_subscription_id(
    client: &Client,
    channel_id: &str,
    access_token: &str,
) -> Result<Option<String>, String> {
    let resp = client
        .get("https://www.googleapis.com/youtube/v3/subscriptions")
        .header("Authorization", format!("Bearer {}", access_token))
        .query(&[
            ("part", "id"),
            ("mine", "true"),
            ("forChannelId", channel_id),
            ("maxResults", "50"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!(
            "Subscriptions.list failed with {}",
            resp.status().as_u16()
        ));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if let Some(items) = json.get("items").and_then(|i| i.as_array()) {
        if let Some(item) = items.first() {
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                return Ok(Some(id.to_string()));
            }
        }
    }
    Ok(None)
}

/// YouTube Data API v3: subscriptions.delete — как в new_endpoints/subscribe_innertube.py.
async fn delete_subscription(
    client: &Client,
    subscription_id: &str,
    access_token: &str,
) -> Result<(), String> {
    let resp = client
        .delete("https://www.googleapis.com/youtube/v3/subscriptions")
        .header("Authorization", format!("Bearer {}", access_token))
        .query(&[("id", subscription_id)])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status_code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Err(format!("Delete failed {}: {}", status_code, text))
    }
}

/// YouTube Data API v3: videos.rate — как в new_endpoints/youtube_rate.py.
async fn rate_video_api(
    client: &Client,
    video_id: &str,
    rating: &str,
    access_token: &str,
) -> Result<(), String> {
    let resp = client
        .post("https://www.googleapis.com/youtube/v3/videos/rate")
        .header("Authorization", format!("Bearer {}", access_token))
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .query(&[("id", video_id), ("rating", rating)])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        Ok(())
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(format!(
            "YouTube rate endpoint returned {}: {}",
            status.as_u16(),
            text
        ))
    }
}

/// YouTube Data API v3: videos.getRating — как в new_endpoints/check_rating.py.
async fn get_rating_api(
    client: &Client,
    video_id: &str,
    access_token: &str,
) -> Result<String, String> {
    let resp = client
        .get("https://www.googleapis.com/youtube/v3/videos/getRating")
        .header("Authorization", format!("Bearer {}", access_token))
        .query(&[("id", video_id)])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "YouTube getRating returned {}: {}",
            status.as_u16(),
            text
        ));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
        if let Some(first) = items.first() {
            if let Some(r) = first.get("rating").and_then(|v| v.as_str()) {
                return Ok(r.to_string());
            }
        }
    }
    Err("No rating info returned for the given video id".to_string())
}

fn validate_rating(value: &str) -> bool {
    matches!(value.to_lowercase().as_str(), "like" | "dislike" | "none")
}

#[utoipa::path(
    get,
    path = "/actions/subscribe",
    params(
        ("channel" = String, Query, description = "Channel handle, URL or UC id"),
        ("token" = String, Query, description = "OAuth refresh token")
    ),
    responses(
        (status = 200, description = "Subscribed to channel", body = YoutubeActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Authentication error")
    )
)]
pub async fn subscribe(
    payload: web::Query<YoutubeSubscriptionRequest>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let request = payload.into_inner();
    let access_token = match obtain_access_token(&request.token, &auth_config).await {
        Ok(token) => token,
        Err(err) => return err,
    };

    let client = Client::new();
    let channel_id = match resolve_channel_id(&request.channel, &client).await {
        Ok(id) => id,
        Err(err) => return error_json(ActixStatusCode::BAD_REQUEST, err),
    };

    if let Err(err) = subscribe_channel_api(&client, &channel_id, &access_token).await {
        return error_json(ActixStatusCode::BAD_GATEWAY, err);
    }

    HttpResponse::Ok().json(YoutubeActionResponse {
        status: "success".to_string(),
        action: "subscribe".to_string(),
        channel_id: Some(channel_id),
        video_id: None,
        message: "Subscribed to channel".to_string(),
    })
}

#[utoipa::path(
    get,
    path = "/actions/unsubscribe",
    params(
        ("channel" = String, Query, description = "Channel handle, URL or UC id"),
        ("token" = String, Query, description = "OAuth refresh token")
    ),
    responses(
        (status = 200, description = "Unsubscribed from channel", body = YoutubeActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Authentication error"),
        (status = 404, description = "Subscription not found")
    )
)]
pub async fn unsubscribe(
    payload: web::Query<YoutubeSubscriptionRequest>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let request = payload.into_inner();
    let access_token = match obtain_access_token(&request.token, &auth_config).await {
        Ok(token) => token,
        Err(err) => return err,
    };

    let client = Client::new();
    let channel_id = match resolve_channel_id(&request.channel, &client).await {
        Ok(id) => id,
        Err(err) => return error_json(ActixStatusCode::BAD_REQUEST, err),
    };

    let subscription_id = match find_subscription_id(&client, &channel_id, &access_token).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return error_json(
                ActixStatusCode::NOT_FOUND,
                "No active subscription found for the channel",
            );
        }
        Err(err) => return error_json(ActixStatusCode::BAD_GATEWAY, err),
    };

    if let Err(err) = delete_subscription(&client, &subscription_id, &access_token).await {
        return error_json(ActixStatusCode::BAD_GATEWAY, err);
    }

    HttpResponse::Ok().json(YoutubeActionResponse {
        status: "success".to_string(),
        action: "unsubscribe".to_string(),
        channel_id: Some(channel_id),
        video_id: None,
        message: "Unsubscribed from channel".to_string(),
    })
}

#[utoipa::path(
    get,
    path = "/actions/rate",
    params(
        ("video_id" = String, Query, description = "YouTube video id"),
        ("rating" = String, Query, description = "like | dislike | none"),
        ("token" = String, Query, description = "OAuth refresh token")
    ),
    responses(
        (status = 200, description = "Video rated", body = YoutubeActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Authentication error")
    )
)]
pub async fn rate(
    payload: web::Query<YoutubeRateRequest>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let request = payload.into_inner();
    if !validate_rating(&request.rating) {
        return error_json(
            ActixStatusCode::BAD_REQUEST,
            "Rating must be one of: like, dislike, none",
        );
    }

    let access_token = match obtain_access_token(&request.token, &auth_config).await {
        Ok(token) => token,
        Err(err) => return err,
    };

    let client = Client::new();
    if let Err(err) =
        rate_video_api(&client, &request.video_id, &request.rating, &access_token).await
    {
        return error_json(ActixStatusCode::BAD_GATEWAY, err);
    }

    HttpResponse::Ok().json(YoutubeActionResponse {
        status: "success".to_string(),
        action: "rate".to_string(),
        channel_id: None,
        video_id: Some(request.video_id),
        message: format!("Video rated {}", request.rating),
    })
}

#[utoipa::path(
    get,
    path = "/actions/check_rating",
    params(
        ("video_id" = String, Query, description = "YouTube video id"),
        ("token" = String, Query, description = "OAuth refresh token")
    ),
    responses(
        (status = 200, description = "Current rating for the video", body = RatingCheckResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Authentication error")
    )
)]
pub async fn check_rating(
    payload: web::Query<RatingCheckRequest>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let request = payload.into_inner();
    if request.video_id.trim().is_empty() || request.token.trim().is_empty() {
        return error_json(
            ActixStatusCode::BAD_REQUEST,
            "video_id and token are required",
        );
    }

    let access_token = match obtain_access_token(&request.token, &auth_config).await {
        Ok(token) => token,
        Err(err) => return err,
    };
    let client = Client::new();
    match get_rating_api(&client, &request.video_id, &access_token).await {
        Ok(rating) => HttpResponse::Ok().json(RatingCheckResponse {
            status: "success".to_string(),
            video_id: request.video_id,
            rating,
        }),
        Err(err) => error_json(ActixStatusCode::BAD_GATEWAY, err),
    }
}

#[utoipa::path(
    get,
    path = "/actions/check_subscription",
    params(
        ("channel" = String, Query, description = "Channel handle, URL or UC id"),
        ("token" = String, Query, description = "OAuth refresh token")
    ),
    responses(
        (status = 200, description = "Subscription status", body = SubscriptionCheckResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Authentication error"),
        (status = 404, description = "Channel not found")
    )
)]
pub async fn check_subscription(
    payload: web::Query<SubscriptionCheckRequest>,
    auth_config: web::Data<AuthConfig>,
) -> impl Responder {
    let request = payload.into_inner();
    if request.channel.trim().is_empty() || request.token.trim().is_empty() {
        return error_json(
            ActixStatusCode::BAD_REQUEST,
            "channel and token are required",
        );
    }

    let access_token = match obtain_access_token(&request.token, &auth_config).await {
        Ok(token) => token,
        Err(err) => return err,
    };
    let client = Client::new();
    let channel_id = match resolve_channel_id(&request.channel, &client).await {
        Ok(id) => id,
        Err(err) => return error_json(ActixStatusCode::NOT_FOUND, err),
    };

    match find_subscription_id(&client, &channel_id, &access_token).await {
        Ok(Some(_)) => HttpResponse::Ok().json(SubscriptionCheckResponse {
            status: "success".to_string(),
            channel_id,
            subscribed: true,
        }),
        Ok(None) => HttpResponse::Ok().json(SubscriptionCheckResponse {
            status: "success".to_string(),
            channel_id,
            subscribed: false,
        }),
        Err(err) => error_json(ActixStatusCode::BAD_GATEWAY, err),
    }
}
