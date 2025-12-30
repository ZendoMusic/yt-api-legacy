use actix_web::{web, HttpResponse, Responder, HttpRequest};
use serde::{Serialize, Deserialize};
use utoipa::ToSchema;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use qrcode::QrCode;
use uuid::Uuid;
use image::{DynamicImage, Rgb, RgbImage};
use base64::{Engine as _, engine::general_purpose};
use reqwest;
use tokio;

pub struct TokenStore {
    tokens: Arc<Mutex<HashMap<String, String>>>,
}

impl TokenStore {
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn store_token(&self, session_id: String, token: String) {
        let mut tokens = self.tokens.lock().unwrap();
        tokens.insert(session_id, token);
    }

    pub fn get_token(&self, session_id: &str) -> Option<String> {
        let tokens = self.tokens.lock().unwrap();
        tokens.get(session_id).cloned()
    }

    pub fn remove_token(&self, session_id: &str) -> Option<String> {
        let mut tokens = self.tokens.lock().unwrap();
        tokens.remove(session_id)
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AccountInfoResponse {
    pub google_account: GoogleAccount,
    #[schema(nullable = true)]
    pub youtube_channel: Option<YouTubeChannel>,
}

#[derive(Serialize, ToSchema)]
pub struct GoogleAccount {
    #[schema(nullable = true)]
    pub id: Option<String>,
    #[schema(nullable = true)]
    pub name: Option<String>,
    #[schema(nullable = true)]
    pub given_name: Option<String>,
    #[schema(nullable = true)]
    pub family_name: Option<String>,
    #[schema(nullable = true)]
    pub email: Option<String>,
    #[schema(nullable = true)]
    pub verified_email: Option<bool>,
    #[schema(nullable = true)]
    pub picture: Option<String>,
    #[schema(nullable = true)]
    pub locale: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct YouTubeChannel {
    #[schema(nullable = true)]
    pub id: Option<String>,
    #[schema(nullable = true)]
    pub title: Option<String>,
    #[schema(nullable = true)]
    pub description: Option<String>,
    #[schema(nullable = true)]
    pub custom_url: Option<String>,
    #[schema(nullable = true)]
    pub published_at: Option<String>,
    #[schema(nullable = true)]
    pub thumbnails: Option<serde_json::Value>,
    #[schema(nullable = true)]
    pub country: Option<String>,
    #[schema(nullable = true)]
    pub subscriber_count: Option<String>,
    #[schema(nullable = true)]
    pub video_count: Option<String>,
    #[schema(nullable = true)]
    pub view_count: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: i32,
    refresh_token: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct UserInfoResponse {
    id: String,
    name: String,
    given_name: Option<String>,
    family_name: Option<String>,
    email: Option<String>,
    verified_email: Option<bool>,
    picture: Option<String>,
    locale: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct YouTubeChannelsResponse {
    items: Option<Vec<YouTubeChannelItem>>,
}

#[derive(Serialize, Deserialize)]
struct YouTubeChannelItem {
    id: String,
    snippet: Option<YouTubeChannelSnippet>,
    statistics: Option<YouTubeChannelStatistics>,
}

#[derive(Serialize, Deserialize)]
struct YouTubeChannelSnippet {
    title: Option<String>,
    description: Option<String>,
    customUrl: Option<String>,
    publishedAt: Option<String>,
    thumbnails: Option<serde_json::Value>,
    country: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct YouTubeChannelStatistics {
    subscriberCount: Option<String>,
    videoCount: Option<String>,
    viewCount: Option<String>,
}

pub fn get_auth_url(config: &AuthConfig, session_id: &str) -> String {
    let scope = config.scopes.join(" ");
    let encoded_scope = urlencoding::encode(&scope);
    let redirect_uri = urlencoding::encode(&config.redirect_uri);
    
    format!(
        "https://accounts.google.com/o/oauth2/auth?\
        client_id={}&\
        redirect_uri={}&\
        scope={}&\
        response_type=code&\
        access_type=offline&\
        prompt=consent&\
        state={}",
        config.client_id,
        redirect_uri,
        encoded_scope,
        session_id
    )
}

fn generate_qr_code(auth_url: &str) -> String {
    let code = QrCode::new(auth_url.as_bytes()).unwrap();
    
    let size = code.width();
    
    let scale = 10;
    let image_size = size * scale;
    let mut img = RgbImage::new(image_size as u32, image_size as u32);
    
    for y in 0..size {
        for x in 0..size {
            let color = if matches!(code[(x, y)], qrcode::Color::Dark) {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            };
            
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = (x * scale + dx) as u32;
                    let py = (y * scale + dy) as u32;
                    img.put_pixel(px, py, color);
                }
            }
        }
    }
    
    let image = DynamicImage::ImageRgb8(img);
    
    let mut buffer = std::io::Cursor::new(Vec::new());
    image.write_to(&mut buffer, image::ImageFormat::Png).unwrap();
    
    general_purpose::STANDARD.encode(&buffer.into_inner())
}

#[utoipa::path(
    get,
    path = "/auth",
    responses(
        (status = 200, description = "Authentication page with QR code or token", body = String)
    )
)]
pub async fn auth_handler(
    req: HttpRequest,
    data: web::Data<AuthConfig>,
    token_store: web::Data<TokenStore>,
) -> impl Responder {
    let session_id = Uuid::new_v4().to_string();
    
    let refresh_token = req.headers().get("refresh_token")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_default();
    
    if !refresh_token.is_empty() {
        let token_display = format!("Token: {}", html_escape::encode_text(&refresh_token));
        return HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(format!("<ytreq>{}</ytreq>", token_display));
    }
    
    if let Some(token) = token_store.get_token(&session_id) {
        if !token.starts_with("Error") {
            token_store.remove_token(&session_id);
            
            let token_display = format!("Token: {}", html_escape::encode_text(&token));
            return HttpResponse::Ok()
                .content_type("text/html; charset=utf-8")
                .body(format!("<ytreq>{}</ytreq>", token_display));
        }
    }
    
    let auth_url = get_auth_url(&data, &session_id);
    let qr_base64 = generate_qr_code(&auth_url);
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(format!("<ytreq>{}</ytreq>", qr_base64))
}

#[utoipa::path(
    get,
    path = "/auth/events",
    responses(
        (status = 200, description = "Server-Sent Events stream for token updates", body = String)
    )
)]
pub async fn auth_events(
    query: web::Query<HashMap<String, String>>,
    token_store: web::Data<TokenStore>,
) -> impl Responder {
    let session_id = match query.get("session_id") {
        Some(id) => id.clone(),
        None => {
            return HttpResponse::Ok()
                .content_type("text/event-stream")
                .body("data: {\"error\": \"Missing session_id\"}\n\n");
        }
    };
    
    let token_store_clone = token_store.clone();
    let session_id_clone = session_id.clone();
    
    if let Some(token) = token_store_clone.get_token(&session_id_clone) {
        let response = serde_json::json!({"token": token});
        token_store_clone.remove_token(&session_id_clone);
        HttpResponse::Ok()
            .content_type("text/event-stream")
            .body(format!("data: {}\n\n", response))
    } else {
        HttpResponse::Ok()
            .content_type("text/event-stream")
            .body("data: {\"error\": \"Authentication timed out\"}\n\n")
    }
}

#[utoipa::path(
    get,
    path = "/oauth/callback",
    responses(
        (status = 200, description = "OAuth callback page", body = String)
    )
)]
pub async fn oauth_callback(
    query: web::Query<HashMap<String, String>>,
    data: web::Data<AuthConfig>,
    token_store: web::Data<TokenStore>,
) -> impl Responder {
    let code = query.get("code");
    let session_id = query.get("state");
    
    if code.is_none() || session_id.is_none() {
        return HttpResponse::BadRequest()
            .content_type("text/html; charset=utf-8")
            .body(r#"
                <html>
                    <body>
                        <h2>Authentication failed</h2>
                        <p>No authorization code or state received.</p>
                    </body>
                </html>
            "#);
    }
    
    let code = code.unwrap();
    let session_id = session_id.unwrap();
    
    // Exchange code for tokens
    let client = reqwest::Client::new();
    let params = [
        ("code", code.as_str()),
        ("client_id", data.client_id.as_str()),
        ("client_secret", data.client_secret.as_str()),
        ("redirect_uri", data.redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];
    
    let res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await;
    
    match res {
        Ok(response) => {
            if response.status().is_success() {
                let token_response: Result<TokenResponse, _> = response.json().await;
                match token_response {
                    Ok(token_data) => {
                        if let Some(refresh_token) = &token_data.refresh_token {
                            token_store.store_token(session_id.clone(), refresh_token.clone());
                            
                            HttpResponse::Ok()
                                .content_type("text/html; charset=utf-8")
                                .body(r#"
                                    <html>
                                        <body>
                                            <h2>Authentication successful</h2>
                                            <p>You can close this window now and refresh the previous page.</p>
                                            <script>
                                                window.close();
                                            </script>
                                        </body>
                                    </html>
                                "#)
                        } else {
                            // Store access token if no refresh token
                            token_store.store_token(session_id.clone(), token_data.access_token.clone());
                            
                            HttpResponse::Ok()
                                .content_type("text/html; charset=utf-8")
                                .body(r#"
                                    <html>
                                        <body>
                                            <h2>Authentication successful</h2>
                                            <p>You can close this window now and refresh the previous page.</p>
                                            <script>
                                                window.close();
                                            </script>
                                        </body>
                                    </html>
                                "#)
                        }
                    }
                    Err(_) => {
                        token_store.store_token(session_id.clone(), "Error: Failed to parse token response".to_string());
                        HttpResponse::BadRequest()
                            .content_type("text/html; charset=utf-8")
                            .body(r#"
                                <html>
                                    <body>
                                        <h2>Error</h2>
                                        <p>Error parsing token response.</p>
                                    </body>
                                </html>
                            "#)
                    }
                }
            } else {
                token_store.store_token(session_id.clone(), "Error: Failed to get token".to_string());
                HttpResponse::BadRequest()
                    .content_type("text/html; charset=utf-8")
                    .body(r#"
                        <html>
                            <body>
                                <h2>Error</h2>
                                <p>Failed to get token from Google.</p>
                            </body>
                        </html>
                    "#)
            }
        }
        Err(_) => {
            token_store.store_token(session_id.clone(), "Error: Network error".to_string());
            HttpResponse::BadRequest()
                .content_type("text/html; charset=utf-8")
                .body(r#"
                    <html>
                        <body>
                            <h2>Error</h2>
                            <p>Network error occurred while getting token.</p>
                        </body>
                    </html>
                "#)
        }
    }
}

#[utoipa::path(
    get,
    path = "/account_info",
    params(
        ("token" = String, Query, description = "Refresh token for Google account")
    ),
    responses(
        (status = 200, description = "Account information", body = AccountInfoResponse),
        (status = 400, description = "Missing token parameter"),
        (status = 401, description = "Invalid refresh token"),
        (status = 500, description = "Failed to get account information")
    )
)]
pub async fn account_info(
    query: web::Query<HashMap<String, String>>,
    data: web::Data<AuthConfig>,
) -> impl Responder {
    let refresh_token = query.get("token");
    
    if refresh_token.is_none() {
        return HttpResponse::BadRequest()
            .json(serde_json::json!({
                "error": "Missing token parameter. Use ?token=YOUR_REFRESH_TOKEN"
            }));
    }
    
    let refresh_token = refresh_token.unwrap();
    
    // Get access token from refresh token
    let client = reqwest::Client::new();
    let params = [
        ("client_id", data.client_id.as_str()),
        ("client_secret", data.client_secret.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    
    let res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await;
    
    let access_token = match res {
        Ok(response) => {
            if response.status().is_success() {
                let token_response: Result<TokenResponse, _> = response.json().await;
                match token_response {
                    Ok(token_data) => token_data.access_token,
                    Err(_) => {
                        return HttpResponse::Unauthorized()
                            .json(serde_json::json!({
                                "error": "Invalid refresh token",
                                "details": "Failed to parse token response"
                            }));
                    }
                }
            } else {
                return HttpResponse::Unauthorized()
                    .json(serde_json::json!({
                        "error": "Invalid refresh token",
                        "details": "Failed to refresh token"
                    }));
            }
        }
        Err(_) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({
                    "error": "Failed to get account information",
                    "details": "Network error occurred while refreshing token"
                }));
        }
    };
    
    // Get user info
    let user_info_res = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await;
    
    let user_info = match user_info_res {
        Ok(response) => {
            if response.status().is_success() {
                let user_info_response: Result<UserInfoResponse, _> = response.json().await;
                match user_info_response {
                    Ok(info) => info,
                    Err(_) => {
                        return HttpResponse::InternalServerError()
                            .json(serde_json::json!({
                                "error": "Failed to get account information",
                                "details": "Failed to parse user info response"
                            }));
                    }
                }
            } else {
                return HttpResponse::InternalServerError()
                    .json(serde_json::json!({
                        "error": "Failed to get account information",
                        "details": "Failed to get user info"
                    }));
            }
        }
        Err(_) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({
                    "error": "Failed to get account information",
                    "details": "Network error occurred while getting user info"
                }));
        }
    };
    
    // Get YouTube channel info
    let youtube_res = client
        .get("https://www.googleapis.com/youtube/v3/channels?part=snippet,statistics&mine=true")
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await;
    
    let youtube_channel = match youtube_res {
        Ok(response) => {
            if response.status().is_success() {
                let youtube_response: Result<YouTubeChannelsResponse, _> = response.json().await;
                match youtube_response {
                    Ok(channels) => {
                        if let Some(items) = channels.items {
                            if !items.is_empty() {
                                let channel_item = &items[0];
                                let snippet = &channel_item.snippet;
                                let statistics = &channel_item.statistics;
                                
                                Some(YouTubeChannel {
                                    id: Some(channel_item.id.clone()),
                                    title: snippet.as_ref().and_then(|s| s.title.clone()),
                                    description: snippet.as_ref().and_then(|s| s.description.clone()),
                                    custom_url: snippet.as_ref().and_then(|s| s.customUrl.clone()),
                                    published_at: snippet.as_ref().and_then(|s| s.publishedAt.clone()),
                                    thumbnails: snippet.as_ref().and_then(|s| s.thumbnails.clone()),
                                    country: snippet.as_ref().and_then(|s| s.country.clone()),
                                    subscriber_count: statistics.as_ref().and_then(|s| s.subscriberCount.clone()),
                                    video_count: statistics.as_ref().and_then(|s| s.videoCount.clone()),
                                    view_count: statistics.as_ref().and_then(|s| s.viewCount.clone()),
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    Err(_) => None
                }
            } else {
                None
            }
        }
        Err(_) => None
    };
    
    let response = AccountInfoResponse {
        google_account: GoogleAccount {
            id: Some(user_info.id),
            name: Some(user_info.name),
            given_name: user_info.given_name,
            family_name: user_info.family_name,
            email: user_info.email,
            verified_email: user_info.verified_email,
            picture: user_info.picture,
            locale: user_info.locale,
        },
        youtube_channel,
    };
    
    HttpResponse::Ok().json(response)
}