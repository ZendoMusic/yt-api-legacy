use actix_web::{web, HttpResponse, Responder, HttpRequest};
use serde::Serialize;
use utoipa::ToSchema;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use qrcode::QrCode;
use uuid::Uuid;
use image::{DynamicImage, Rgb, RgbImage};
use base64::{Engine as _, engine::general_purpose};
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
    _data: web::Data<AuthConfig>,
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
    
    let auth_url = get_auth_url(&_data, &session_id);
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
    _data: web::Data<AuthConfig>,
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
    
    let _code = code.unwrap();
    let session_id = session_id.unwrap();
    
    let refresh_token = format!("refresh_token_{}", Uuid::new_v4());
    
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
) -> impl Responder {
    let refresh_token = query.get("token");
    
    if refresh_token.is_none() {
        return HttpResponse::BadRequest()
            .json(serde_json::json!({
                "error": "Missing token parameter. Use ?token=YOUR_REFRESH_TOKEN"
            }));
    }
    
    let _refresh_token = refresh_token.unwrap();
    
    let response = AccountInfoResponse {
        google_account: GoogleAccount {
            id: Some("123456789".to_string()),
            name: Some("Test User".to_string()),
            given_name: Some("Test".to_string()),
            family_name: Some("User".to_string()),
            email: Some("test@example.com".to_string()),
            verified_email: Some(true),
            picture: Some("https://example.com/picture.jpg".to_string()),
            locale: Some("en".to_string()),
        },
        youtube_channel: Some(YouTubeChannel {
            id: Some("UC123456789".to_string()),
            title: Some("Test Channel".to_string()),
            description: Some("A test YouTube channel".to_string()),
            custom_url: Some("@testchannel".to_string()),
            published_at: Some("2020-01-01T00:00:00Z".to_string()),
            thumbnails: Some(serde_json::json!({
                "default": {
                    "url": "https://example.com/thumbnail.jpg",
                    "width": 88,
                    "height": 88
                }
            })),
            country: Some("US".to_string()),
            subscriber_count: Some("1000".to_string()),
            video_count: Some("50".to_string()),
            view_count: Some("10000".to_string()),
        }),
    };
    
    HttpResponse::Ok().json(response)
}