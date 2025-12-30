use actix_web::{web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
pub struct YouTubeSession {
    pub device_id: String,
    pub username: String,
    pub password: String,
    pub access_token: String,
    pub refresh_token: String,
    pub is_linked: bool,
}

#[derive(Serialize, ToSchema)]
pub struct IsUsernameTakeResult {
    pub status: bool,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct OAuth2TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: i32,
    refresh_token: String,
}

#[derive(Serialize, ToSchema)]
pub struct OAuth2UserInfoResponse {
    id: String,
    name: String,
    email: String,
    verified_email: bool,
}

const TOKENS_FILE_PATH: &str = "assets/tokens.json";

fn load_sessions() -> Vec<YouTubeSession> {
    if let Ok(content) = fs::read_to_string(TOKENS_FILE_PATH) {
        if let Ok(sessions) = serde_json::from_str(&content) {
            sessions
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    }
}

fn save_sessions(sessions: &Vec<YouTubeSession>) -> Result<(), std::io::Error> {
    // Create directory if it doesn't exist
    if let Some(parent) = Path::new(TOKENS_FILE_PATH).parent() {
        fs::create_dir_all(parent)?;
    }
    
    let json = serde_json::to_string_pretty(sessions)?;
    let mut file = File::create(TOKENS_FILE_PATH)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

fn is_username_taken(username: &str) -> bool {
    let sessions = load_sessions();
    sessions.iter().any(|s| s.username == username)
}

fn is_username_password_correct(username: &str, password: &str) -> bool {
    let sessions = load_sessions();
    sessions.iter().any(|s| s.username == username && s.password == password)
}

fn get_valid_login_data(username: &str, password: &str) -> Option<(String, String)> {
    let sessions = load_sessions();
    sessions.iter()
        .find(|s| s.username == username && s.password == password && s.is_linked)
        .map(|s| (s.device_id.clone(), s.access_token.clone()))
}

#[utoipa::path(
    get,
    path = "/check_if_username_is_taken",
    params(
        ("username" = String, Query, description = "Username to check")
    ),
    responses(
        (status = 200, description = "Check if username is taken", body = IsUsernameTakeResult)
    )
)]
pub async fn check_if_username_is_taken(
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    let username = match query.get("username") {
        Some(u) => u,
        None => {
            return HttpResponse::BadRequest().body("Must have a username parm.");
        }
    };

    let response = IsUsernameTakeResult {
        status: is_username_taken(username),
    };

    HttpResponse::Ok().json(response)
}

#[utoipa::path(
    post,
    path = "/link_device_token",
    request_body = String,
    responses(
        (status = 200, description = "Device linked successfully"),
        (status = 400, description = "Bad request")
    )
)]
pub async fn link_device_token(
    body: web::Bytes,
) -> impl Responder {
    // Parse JSON body
    let json_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            return HttpResponse::BadRequest().body("Invalid UTF-8 in request body");
        }
    };

    let data: HashMap<String, serde_json::Value> = match serde_json::from_str(json_str) {
        Ok(d) => d,
        Err(_) => {
            return HttpResponse::BadRequest().body("Invalid JSON in request body");
        }
    };

    let device_id = match data.get("device_id") {
        Some(id) => match id.as_str() {
            Some(s) => s.to_string(),
            None => {
                return HttpResponse::BadRequest().body("device_id must be a string");
            }
        },
        None => {
            return HttpResponse::BadRequest().body("Missing device_id");
        }
    };

    let username = data.get("username").and_then(|u| u.as_str()).unwrap_or("").to_string();
    let password = data.get("password").and_then(|p| p.as_str()).unwrap_or("").to_string();
    let access_token = data.get("access_token").and_then(|a| a.as_str()).unwrap_or("").to_string();
    let refresh_token = data.get("refresh_token").and_then(|r| r.as_str()).unwrap_or("").to_string();

    // Load existing sessions
    let mut sessions = load_sessions();

    // Check if username is already taken
    if !username.is_empty() && sessions.iter().any(|s| s.username == username) {
        return HttpResponse::BadRequest().body("Username taken");
    }

    // Find existing session with this device_id
    let existing_session = sessions.iter_mut().find(|s| s.device_id == device_id);

    if let Some(session) = existing_session {
        // Update existing session
        if !session.is_linked {
            session.username = username;
            session.password = password;
            session.access_token = access_token;
            session.refresh_token = refresh_token;
            session.is_linked = true; // Simplified - in real implementation would validate token
        }
    } else {
        // Create new session
        let new_session = YouTubeSession {
            device_id,
            username,
            password,
            access_token,
            refresh_token,
            is_linked: true, // Simplified - in real implementation would validate token
        };
        sessions.push(new_session);
    }

    // Save sessions
    if let Err(_) = save_sessions(&sessions) {
        return HttpResponse::InternalServerError().body("Failed to save sessions");
    }

    HttpResponse::Ok().body("Device linked")
}

#[utoipa::path(
    post,
    path = "/get_session",
    responses(
        (status = 200, description = "Get session information"),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_session(
    body: web::Bytes,
) -> impl Responder {
    // Parse form data
    let form_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            return HttpResponse::BadRequest().body("Invalid UTF-8 in request body");
        }
    };

    let mut form_data = HashMap::new();
    for pair in form_str.split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            let decoded_key = urlencoding::decode(key).unwrap_or(key.into()).to_string();
            let decoded_value = urlencoding::decode(value).unwrap_or(value.into()).to_string();
            form_data.insert(decoded_key, decoded_value);
        }
    }

    let username = match form_data.get("username") {
        Some(u) => u,
        None => {
            return HttpResponse::BadRequest().body("Must have a username parm.");
        }
    };

    let password = match form_data.get("password") {
        Some(p) => p,
        None => {
            return HttpResponse::BadRequest().body("Must have a password parm.");
        }
    };

    if !is_username_password_correct(username, password) {
        return HttpResponse::Unauthorized().body("");
    }

    let sessions = load_sessions();
    let session = sessions.iter().find(|s| s.username == *username);

    match session {
        Some(s) => HttpResponse::Ok().json(s),
        None => HttpResponse::Unauthorized().body(""),
    }
}

#[utoipa::path(
    post,
    path = "/accounts/ClientLogin",
    responses(
        (status = 200, description = "Client login response")
    )
)]
pub async fn client_login(
    body: web::Bytes,
) -> impl Responder {
    // Parse form data
    let form_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            return HttpResponse::BadRequest().body("Invalid UTF-8 in request body");
        }
    };

    let mut form_data = HashMap::new();
    for pair in form_str.split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            let decoded_key = urlencoding::decode(key).unwrap_or(key.into()).to_string();
            let decoded_value = urlencoding::decode(value).unwrap_or(value.into()).to_string();
            form_data.insert(decoded_key, decoded_value);
        }
    }

    let username = form_data.get("Email").cloned().unwrap_or_default();
    let password = form_data.get("Passwd").cloned().unwrap_or_default();

    if username.is_empty() && password.is_empty() {
        return HttpResponse::Ok().body("You must have a username and password!");
    }

    if !is_username_password_correct(&username, &password) {
        return HttpResponse::Ok().body("Your username and password are wrong, or your account isn't linked!!!");
    }

    match get_valid_login_data(&username, &password) {
        Some((device_id, _)) => {
            let response = format!("SID={}\nLSID={}\nAuth={}\n", device_id, device_id, device_id);
            HttpResponse::Ok().content_type("text/plain").body(response)
        },
        None => HttpResponse::Ok().body("Your username and password are wrong, or your account isn't linked!!!"),
    }
}

#[utoipa::path(
    post,
    path = "/youtube/accounts/ClientLogin",
    responses(
        (status = 200, description = "Client login response")
    )
)]
pub async fn youtube_client_login(
    body: web::Bytes,
) -> impl Responder {
    // This is the same as client_login
    client_login(body).await
}

#[utoipa::path(
    post,
    path = "/o/oauth2/token",
    request_body = String,
    responses(
        (status = 200, description = "OAuth token response", body = OAuth2TokenResponse)
    )
)]
pub async fn oauth2_token(
    body: web::Bytes,
) -> impl Responder {
    // Parse form data
    let form_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            return HttpResponse::BadRequest().body("Invalid UTF-8 in request body");
        }
    };

    let mut form_data = HashMap::new();
    for pair in form_str.split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            let decoded_key = urlencoding::decode(key).unwrap_or(key.into()).to_string();
            let decoded_value = urlencoding::decode(value).unwrap_or(value.into()).to_string();
            form_data.insert(decoded_key, decoded_value);
        }
    }

    let code = form_data.get("code").cloned();
    let refresh_token = form_data.get("refresh_token").cloned();
    
    let access_token = code.or(refresh_token).unwrap_or_else(|| "lifeisstrange".to_string());

    let response = OAuth2TokenResponse {
        access_token: access_token.clone(),
        token_type: "Bearer".to_string(),
        expires_in: 3600,
        refresh_token: access_token,
    };

    HttpResponse::Ok().json(response)
}

#[utoipa::path(
    get,
    path = "/oauth2/v1/userinfo",
    responses(
        (status = 200, description = "User info response", body = OAuth2UserInfoResponse)
    )
)]
pub async fn oauth2_userinfo() -> impl Responder {
    let response = OAuth2UserInfoResponse {
        id: "2013".to_string(),
        name: "David Price Is My Bea".to_string(),
        email: "ilovemenandwomenandenbies@gmail.com".to_string(),
        verified_email: true,
    };

    HttpResponse::Ok().json(response)
}