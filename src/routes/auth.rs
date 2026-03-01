use actix_web::{web, HttpResponse, Responder, HttpRequest};
use serde::{Serialize, Deserialize};
use utoipa::ToSchema;
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use base64::{Engine as _, engine::general_purpose};
use reqwest;
use actix_web::cookie::{Cookie, SameSite};

#[derive(Clone)]
pub struct DeviceFlowData {
    pub device_code: String,
    pub user_code: String,
    pub qr_base64: String,
}

pub struct TokenStore {
    tokens: Arc<Mutex<HashMap<String, String>>>,
    device_flows: Arc<Mutex<HashMap<String, DeviceFlowData>>>,
}

impl TokenStore {
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(Mutex::new(HashMap::new())),
            device_flows: Arc::new(Mutex::new(HashMap::new())),
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

    pub fn store_device_flow(&self, session_id: String, data: DeviceFlowData) {
        let mut flows = self.device_flows.lock().unwrap();
        flows.insert(session_id, data);
    }

    pub fn get_device_flow(&self, session_id: &str) -> Option<DeviceFlowData> {
        let flows = self.device_flows.lock().unwrap();
        flows.get(session_id).cloned()
    }

    pub fn remove_device_flow(&self, session_id: &str) -> Option<DeviceFlowData> {
        let mut flows = self.device_flows.lock().unwrap();
        flows.remove(session_id)
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub youtube_api_key: String,
}

#[derive(Serialize, ToSchema)]
pub struct AccountInfoResponse {
    pub google_account: GoogleAccount,
    #[schema(nullable = true)]
    pub youtube_channel: Option<YouTubeChannel>,
}

// Структуры для парсинга ответа от YouTubei API
#[derive(Deserialize)]
struct AccountsListResponse {
    contents: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct AccountItem {
    #[serde(rename = "accountItem")]
    account_item: Option<AccountItemData>,
}

#[derive(Deserialize)]
struct AccountItemData {
    #[serde(rename = "accountName")]
    account_name: Option<SimpleText>,
    #[serde(rename = "accountByline")]
    account_byline: Option<SimpleText>,
    #[serde(rename = "channelHandle")]
    channel_handle: Option<SimpleText>,
    #[serde(rename = "hasChannel")]
    has_channel: Option<bool>,
    #[serde(rename = "isSelected")]
    is_selected: Option<bool>,
    #[serde(rename = "accountPhoto")]
    account_photo: Option<AccountPhoto>,
    #[serde(rename = "serviceEndpoint")]
    service_endpoint: Option<ServiceEndpoint>,
}

#[derive(Deserialize)]
struct SimpleText {
    #[serde(rename = "simpleText")]
    simple_text: Option<String>,
}

#[derive(Deserialize)]
struct AccountPhoto {
    thumbnails: Option<Vec<Thumbnail>>,
}

#[derive(Deserialize)]
struct Thumbnail {
    url: Option<String>,
}

#[derive(Deserialize)]
struct ServiceEndpoint {
    #[serde(rename = "selectActiveIdentityEndpoint")]
    select_active_identity_endpoint: Option<SelectActiveIdentityEndpoint>,
}

#[derive(Deserialize)]
struct SelectActiveIdentityEndpoint {
    #[serde(rename = "supportedTokens")]
    supported_tokens: Option<Vec<SupportedToken>>,
}

#[derive(Deserialize)]
struct SupportedToken {
    #[serde(rename = "accountStateToken")]
    account_state_token: Option<AccountStateToken>,
}

#[derive(Deserialize)]
struct AccountStateToken {
    #[serde(rename = "obfuscatedGaiaId")]
    obfuscated_gaia_id: Option<String>,
}

// Старые структуры оставляем для обратной совместимости, но они больше не используются
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

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct DeviceTokenResponse {
    access_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct MdxHandoffRequest {
    context: MdxContext,
    handoff_qr_params: MdxHandoffQrParams,
}

#[derive(Serialize, Deserialize)]
struct MdxContext {
    client: MdxClient,
}

#[derive(Serialize, Deserialize)]
struct MdxClient {
    #[serde(rename = "clientName")]
    client_name: String,
    #[serde(rename = "clientVersion")]
    client_version: String,
    #[serde(rename = "deviceMake")]
    device_make: String,
    #[serde(rename = "deviceModel")]
    device_model: String,
    platform: String,
    hl: String,
    gl: String,
}

#[derive(Serialize, Deserialize)]
struct MdxHandoffQrParams {
    #[serde(rename = "rapidQrParams")]
    rapid_qr_params: MdxRapidQrParams,
}

#[derive(Serialize, Deserialize)]
struct MdxRapidQrParams {
    #[serde(rename = "qrPresetStyle")]
    qr_preset_style: String,
    #[serde(rename = "userCode")]
    user_code: String,
    #[serde(rename = "rapidQrFeature")]
    rapid_qr_feature: String,
}

#[derive(Deserialize)]
struct MdxHandoffResponse {
    #[serde(rename = "rapidQrRenderer")]
    rapid_qr_renderer: Option<MdxRapidQrRenderer>,
}

#[derive(Deserialize)]
struct MdxRapidQrRenderer {
    #[serde(rename = "qrCodeRenderer")]
    qr_code_renderer: MdxQrCodeRenderer,
}

#[derive(Deserialize)]
struct MdxQrCodeRenderer {
    #[serde(rename = "qrCodeImage")]
    qr_code_image: MdxQrCodeImage,
}

#[derive(Deserialize)]
struct MdxQrCodeImage {
    thumbnails: Vec<MdxThumbnail>,
}

#[derive(Deserialize)]
struct MdxThumbnail {
    url: String,
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

async fn get_device_code(
    client: &reqwest::Client,
    client_id: &str,
    device_id: &str,
) -> Result<DeviceCodeResponse, Box<dyn std::error::Error>> {
    let params = [
        ("client_id", client_id),
        ("scope", "http://gdata.youtube.com https://www.googleapis.com/auth/youtube-paid-content"),
        ("device_id", device_id),
        ("device_model", "ytlr:samsung:smarttv"),
    ];

    let response = client
        .post("https://www.youtube.com/o/oauth2/device/code")
        .header("User-Agent", "Mozilla/5.0 (SMART-TV; Linux; Tizen 6.0)")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await?;

    let device_code_response: DeviceCodeResponse = response.json().await?;
    Ok(device_code_response)
}

async fn get_tv_qr(
    client: &reqwest::Client,
    user_code: &str,
    api_key: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let payload = MdxHandoffRequest {
        context: MdxContext {
            client: MdxClient {
                client_name: "TVHTML5".to_string(),
                client_version: "7.20251217.19.00".to_string(),
                device_make: "Samsung".to_string(),
                device_model: "SmartTV".to_string(),
                platform: "TV".to_string(),
                hl: "ru".to_string(),
                gl: "RU".to_string(),
            },
        },
        handoff_qr_params: MdxHandoffQrParams {
            rapid_qr_params: MdxRapidQrParams {
                qr_preset_style: "HANDOFF_QR_LIMITED_PRESET_STYLE_MODERN_BIG_DOTS_INVERT_WITH_YT_LOGO".to_string(),
                user_code: user_code.to_string(),
                rapid_qr_feature: "RAPID_QR_FEATURE_DEFAULT".to_string(),
            },
        },
    };

    let response = client
        .post(&format!("https://www.youtube.com/youtubei/v1/mdx/handoff?key={}", api_key))
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0 (SMART-TV; Linux; Tizen 6.0)")
        .json(&payload)
        .send()
        .await?;

    let mdx_response: MdxHandoffResponse = response.json().await?;

    if let Some(rapid_qr_renderer) = mdx_response.rapid_qr_renderer {
        let url = &rapid_qr_renderer.qr_code_renderer.qr_code_image.thumbnails[0].url;
        // URL может быть в формате data:image/png;base64,{base64_data}
        if let Some(b64_data) = url.split(',').nth(1) {
            let qr_bytes = general_purpose::STANDARD.decode(b64_data)?;
            return Ok(qr_bytes);
        }
    }

    Err("QR not returned".into())
}

async fn check_device_token(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    device_code: &str,
) -> Result<DeviceTokenResponse, Box<dyn std::error::Error>> {
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", device_code),
        ("grant_type", "http://oauth.net/grant_type/device/1.0"),
    ];

    let response = client
        .post("https://www.youtube.com/o/oauth2/token")
        .header("User-Agent", "Mozilla/5.0 (SMART-TV; Linux; Tizen 6.0)")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await?;

    let token_response: DeviceTokenResponse = response.json().await?;
    Ok(token_response)
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

/// Serves the login page (Google account sign-in) that works through /auth.
pub async fn auth_login_page() -> impl Responder {
    let html = fs::read_to_string("assets/html/login.html")
        .unwrap_or_else(|_| {
            r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Sign in</title></head>
<body><h1>Sign in</h1><p><a href="/auth/start">Sign in with Google</a></p></body></html>"#.to_string()
        });
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

/// Redirects to Google OAuth; callback goes to /oauth/callback. Sets session_id cookie.
pub async fn auth_start(
    req: HttpRequest,
    data: web::Data<AuthConfig>,
) -> impl Responder {
    let session_id = req
        .cookie("session_id")
        .map(|c| c.value().to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let auth_url = get_auth_url(&data, &session_id);
    let cookie = Cookie::build("session_id", session_id.clone())
        .path("/")
        .same_site(SameSite::Lax)
        .http_only(false)
        .finish();
    HttpResponse::Found()
        .insert_header(("Location", auth_url))
        .insert_header(("Set-Cookie", cookie.to_string()))
        .finish()
}

#[utoipa::path(
    get,
    path = "/auth",
    params(
        ("check" = Option<String>, Query, description = "Check authentication status"),
        ("type" = Option<String>, Query, description = "Type of authentication: 'pc' for user code, default is QR code")
    ),
    responses(
        (status = 200, description = "QR code (base64) or refresh token or user code", body = String)
    )
)]
pub async fn auth_handler(
    req: HttpRequest,
    query: web::Query<HashMap<String, String>>,
    data: web::Data<AuthConfig>,
    token_store: web::Data<TokenStore>,
) -> impl Responder {
    let session_id = req.cookie("session_id")
        .map(|c| c.value().to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    
    // Check if type=pc is specified to return user code instead of QR code
    let is_pc_type = query.get("type").map_or(false, |t| t == "pc");
    
    // Если передан refresh_token в заголовке, отдаем его
    if let Some(refresh_token_header) = req.headers().get("refresh_token") {
        if let Ok(refresh_token) = refresh_token_header.to_str() {
            if !refresh_token.is_empty() {
                let token_display = format!("Token: {}", html_escape::encode_text(refresh_token));
                return HttpResponse::Ok()
                    .content_type("text/html; charset=utf-8")
                    .body(format!("<ytreq>{}</ytreq>", token_display));
            }
        }
    }
    
    // Если есть готовый токен, отдаем его (не удаляем, чтобы он был доступен при повторных запросах)
    if let Some(token) = token_store.get_token(&session_id) {
        if !token.starts_with("Error") {
            let token_display = format!("Token: {}", html_escape::encode_text(&token));
            let cookie = Cookie::build("session_id", session_id.clone())
                .path("/")
                .same_site(SameSite::Lax)
                .http_only(false)
                .finish();
            return HttpResponse::Ok()
                .insert_header(("Set-Cookie", cookie.to_string()))
                .content_type("text/html; charset=utf-8")
                .body(format!("<ytreq>{}</ytreq>", token_display));
        }
    }
    
    // Если есть активный device flow, проверяем статус авторизации
    // (как в Python скрипте - при каждом запросе проверяется статус)
    if let Some(device_flow) = token_store.get_device_flow(&session_id) {
        let client = reqwest::Client::new();
        match check_device_token(
            &client,
            &data.client_id,
            &data.client_secret,
            &device_flow.device_code,
        ).await {
            Ok(token_response) => {
                if let Some(refresh_token) = token_response.refresh_token {
                    // Токен получен - удаляем device flow и сохраняем токен
                    token_store.remove_device_flow(&session_id);
                    token_store.store_token(session_id.clone(), refresh_token.clone());
                    let token_display = format!("Token: {}", html_escape::encode_text(&refresh_token));
                    let cookie = Cookie::build("session_id", session_id.clone())
                        .path("/")
                        .same_site(SameSite::Lax)
                        .http_only(false)
                        .finish();
                    return HttpResponse::Ok()
                        .insert_header(("Set-Cookie", cookie.to_string()))
                        .content_type("text/html; charset=utf-8")
                        .body(format!("<ytreq>{}</ytreq>", token_display));
                } else if let Some(error) = token_response.error {
                    if error == "authorization_pending" {
                        // Возвращаем сохраненный QR код или user code, в зависимости от типа
                        if is_pc_type {
                            return HttpResponse::Ok()
                                .content_type("text/html; charset=utf-8")
                                .body(format!("<ytreq>{}</ytreq>", device_flow.user_code));
                        } else {
                            return HttpResponse::Ok()
                                .content_type("text/html; charset=utf-8")
                                .body(format!("<ytreq>{}</ytreq>", device_flow.qr_base64));
                        }
                    } else {
                        let error_msg = format!("❌ {}", error);
                        return HttpResponse::Ok()
                            .content_type("text/html; charset=utf-8")
                            .body(format!("<ytreq>{}</ytreq>", error_msg));
                    }
                } else {
                    // Нет ошибки, но и нет токена - возвращаем QR код или user code в зависимости от типа
                    if is_pc_type {
                        return HttpResponse::Ok()
                            .content_type("text/html; charset=utf-8")
                            .body(format!("<ytreq>{}</ytreq>", device_flow.user_code));
                    } else {
                        return HttpResponse::Ok()
                            .content_type("text/html; charset=utf-8")
                            .body(format!("<ytreq>{}</ytreq>", device_flow.qr_base64));
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("❌ Error: {}", e);
                return HttpResponse::Ok()
                    .content_type("text/html; charset=utf-8")
                    .body(format!("<ytreq>{}</ytreq>", error_msg));
            }
        }
    }
    
    // Получение device code и QR (только если device flow еще не начат)
    let device_id = Uuid::new_v4().to_string();
    let client = reqwest::Client::new();
    
    match get_device_code(&client, &data.client_id, &device_id).await {
        Ok(device_code_response) => {
            // Получаем QR код
            match get_tv_qr(&client, &device_code_response.user_code, &data.youtube_api_key).await {
                Ok(qr_bytes) => {
                    // Кодируем QR в base64
                    let qr_base64 = general_purpose::STANDARD.encode(&qr_bytes);
                    
                    let user_code_clone = device_code_response.user_code.clone();
                    
                    // Сохраняем device flow данные вместе с QR кодом
                    token_store.store_device_flow(
                        session_id.clone(),
                        DeviceFlowData {
                            device_code: device_code_response.device_code,
                            user_code: user_code_clone.clone(),
                            qr_base64: qr_base64.clone(),
                        },
                    );
                    
                    let cookie = Cookie::build("session_id", session_id.clone())
                        .path("/")
                        .same_site(SameSite::Lax)
                        .http_only(false)
                        .finish();
                    
                    // Return user code if type=pc, otherwise return QR code
                    let response_content = if is_pc_type {
                        user_code_clone
                    } else {
                        qr_base64.clone()
                    };
                    
                    HttpResponse::Ok()
                        .insert_header(("Set-Cookie", cookie.to_string()))
                        .content_type("text/html; charset=utf-8")
                        .body(format!("<ytreq>{}</ytreq>", response_content))
                }
                Err(e) => {
                    HttpResponse::InternalServerError()
                        .content_type("text/html; charset=utf-8")
                        .body(format!("<ytreq>Error getting QR: {}</ytreq>", e))
                }
            }
        }
        Err(e) => {
            HttpResponse::InternalServerError()
                .content_type("text/html; charset=utf-8")
                .body(format!("<ytreq>Error getting device code: {}</ytreq>", e))
        }
    }
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
    let session_id = query.get("session_id").cloned().unwrap_or_default();
    if session_id.is_empty() {
        return HttpResponse::Ok()
            .content_type("text/event-stream")
            .body("data: {\"error\": \"Missing session_id\"}\n\n");
    }
    
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
                            
                            let cookie = Cookie::build("session_id", session_id.clone())
                                .path("/")
                                .same_site(SameSite::Lax)
                                .http_only(false)
                                .finish();
                            
                            HttpResponse::Ok()
                                .insert_header(("Set-Cookie", cookie.to_string()))
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
                            token_store.store_token(session_id.clone(), token_data.access_token.clone());
                            
                            let cookie = Cookie::build("session_id", session_id.clone())
                                .path("/")
                                .same_site(SameSite::Lax)
                                .http_only(false)
                                .finish();
                            
                            HttpResponse::Ok()
                                .insert_header(("Set-Cookie", cookie.to_string()))
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
        ("token" = Option<String>, Query, description = "Refresh token (optional if session cookie is set)")
    ),
    responses(
        (status = 200, description = "Account information", body = AccountInfoResponse),
        (status = 401, description = "Missing or invalid token"),
        (status = 500, description = "Failed to get account information")
    )
)]
pub async fn account_info(
    req: HttpRequest,
    query: web::Query<HashMap<String, String>>,
    data: web::Data<AuthConfig>,
    token_store: web::Data<TokenStore>,
) -> impl Responder {
    // Token: from query ?token=... or from session (cookie session_id)
    let refresh_token = query.get("token").cloned().or_else(|| {
        req.cookie("session_id")
            .map(|c| c.value().to_string())
            .and_then(|session_id| token_store.get_token(&session_id))
            .filter(|t| !t.is_empty() && !t.starts_with("Error"))
    });

    if refresh_token.is_none() {
        return HttpResponse::Unauthorized()
            .insert_header(("Cache-Control", "no-store, no-cache, must-revalidate"))
            .json(serde_json::json!({
                "error": "Missing or invalid token. Sign in or use ?token=YOUR_REFRESH_TOKEN"
            }));
    }

    let refresh_token = refresh_token.unwrap();
    
    let client = reqwest::Client::new();
    let params = [
        ("client_id", data.client_id.as_str()),
        ("client_secret", data.client_secret.as_str()),
        ("refresh_token", &refresh_token),
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
    
    // Запрос к YouTubei API для получения информации об аккаунте
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "TVHTML5",
                "clientVersion": "7.20251217.19.00",
                "hl": "ru",
                "gl": "RU",
                "platform": "TV"
            },
            "user": {
                "enableSafetyMode": false
            }
        },
        "accountReadMask": {
            "returnOwner": true,
            "returnBrandAccounts": true,
            "returnPersonaAccounts": true,
            "returnFamilyChildAccounts": true,
            "returnFamilyMembersAccounts": false
        }
    });

    let accounts_res = client
        .post("https://www.youtube.com/youtubei/v1/account/accounts_list?prettyPrint=false")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("X-Youtube-Client-Name", "85")
        .header("X-Youtube-Client-Version", "7.20251217.19.00")
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0 (SMART-TV; Tizen 6.0)")
        .json(&body)
        .send()
        .await;

    let accounts_data: serde_json::Value = match accounts_res {
        Ok(response) => {
            if !response.status().is_success() {
                return HttpResponse::InternalServerError()
                    .json(serde_json::json!({
                        "error": "Failed to get account information",
                        "details": format!("HTTP error: {}", response.status())
                    }));
            }
            match response.json().await {
                Ok(data) => data,
                Err(e) => {
                    return HttpResponse::InternalServerError()
                        .json(serde_json::json!({
                            "error": "Failed to get account information",
                            "details": format!("Failed to parse response: {}", e)
                        }));
                }
            }
        }
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({
                    "error": "Failed to get account information",
                    "details": format!("Network error: {}", e)
                }));
        }
    };

    // Парсим ответ по структуре из Python скрипта
    let accounts = accounts_data
        .get("contents")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.get(0))
        .and_then(|item| item.get("accountSectionListRenderer"))
        .and_then(|renderer| renderer.get("contents"))
        .and_then(|contents| contents.as_array())
        .and_then(|arr| arr.get(0))
        .and_then(|item| item.get("accountItemSectionRenderer"))
        .and_then(|renderer| renderer.get("contents"))
        .and_then(|contents| contents.as_array());

    let primary_account = if let Some(accounts_array) = accounts {
        accounts_array
            .iter()
            .find_map(|account| {
                let account_item = account.get("accountItem")?;
                // Основной аккаунт — тот, у кого есть accountByline
                if account_item.get("accountByline").is_some() {
                    Some(account_item)
                } else {
                    None
                }
            })
    } else {
        None
    };

    if primary_account.is_none() {
        return HttpResponse::InternalServerError()
            .json(serde_json::json!({
                "error": "Failed to get account information",
                "details": "Primary account not found"
            }));
    }

    let account = primary_account.unwrap();

    // Извлекаем данные
    let account_name = account
        .get("accountName")
        .and_then(|n| n.get("simpleText"))
        .and_then(|s| s.as_str())
        .unwrap_or("Неизвестно")
        .to_string();

    let email = account
        .get("accountByline")
        .and_then(|b| b.get("simpleText"))
        .and_then(|s| s.as_str())
        .unwrap_or("Не указан")
        .to_string();

    let channel_handle = account
        .get("channelHandle")
        .and_then(|h| h.get("simpleText"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    let has_channel = account
        .get("hasChannel")
        .and_then(|h| h.as_bool())
        .unwrap_or(false);

    let _is_selected = account
        .get("isSelected")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    let photo_url_raw = account
        .get("accountPhoto")
        .and_then(|p| p.get("thumbnails"))
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.last())
        .and_then(|thumb| thumb.get("url"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    let obfuscated_gaia_id = account
        .get("serviceEndpoint")
        .and_then(|se| se.get("selectActiveIdentityEndpoint"))
        .and_then(|sai| sai.get("supportedTokens"))
        .and_then(|st| st.as_array())
        .and_then(|tokens| {
            tokens
                .iter()
                .find_map(|token| {
                    token
                        .get("accountStateToken")
                        .and_then(|ast| ast.get("obfuscatedGaiaId"))
                        .and_then(|id| id.as_str())
                        .map(|s| s.to_string())
                })
        });

    // Получаем base URL для channel_icon из запроса
    let base_url = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .map(|host| {
            let scheme = req
                .uri()
                .scheme_str()
                .unwrap_or("http");
            format!("{}://{}", scheme, host)
        })
        .unwrap_or_else(|| {
            // Fallback на localhost если не можем определить из запроса
            "http://localhost:2823".to_string()
        });

    // Формируем URL для иконки через /channel_icon/
    let picture_url = photo_url_raw.map(|url| {
        format!("{}/channel_icon/{}", base_url, urlencoding::encode(&url))
    });

    // Разбиваем имя на given_name и family_name (если возможно)
    let name_parts: Vec<&str> = account_name.split_whitespace().collect();
    let given_name = name_parts.first().map(|s| s.to_string());
    let family_name = if name_parts.len() > 1 {
        Some(name_parts[1..].join(" "))
    } else {
        None
    };

    // Формируем ответ в старом формате
    let google_account = GoogleAccount {
        id: obfuscated_gaia_id.clone(),
        name: Some(account_name.clone()),
        given_name,
        family_name,
        email: Some(email.clone()),
        verified_email: Some(true), // Предполагаем, что email верифицирован
        picture: picture_url.clone(),
        locale: Some("ru".to_string()), // Из контекста запроса
    };

    // Формируем информацию о канале, если есть
    let youtube_channel = if has_channel {
        Some(YouTubeChannel {
            id: obfuscated_gaia_id.clone(),
            title: Some(account_name),
            description: None,
            custom_url: channel_handle.clone(),
            published_at: None,
            thumbnails: None,
            country: None,
            subscriber_count: None,
            video_count: None,
            view_count: None,
        })
    } else {
        None
    };

    let response = AccountInfoResponse {
        google_account,
        youtube_channel,
    };
    
    HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store, no-cache, must-revalidate"))
        .json(response)
}
