use actix_web::{web, App, HttpResponse, HttpServer, Responder, middleware::Logger};
use actix_files as fs;
use serde::{Deserialize, Serialize};
use std::fs as stdfs;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod config;
use config::Config;
mod check;
mod log;
mod routes;

use routes::auth::{AuthConfig, TokenStore};

#[derive(OpenApi)]
#[openapi(
    paths(
        health_check,
        index,
        routes::auth::auth_handler,
        routes::auth::auth_events,
        routes::auth::oauth_callback,
        routes::auth::account_info,
        routes::search::get_top_videos,
        routes::search::get_search_videos,
        routes::search::get_search_suggestions,
    ),
    components(
        schemas(
            Config,
            routes::auth::AccountInfoResponse,
            routes::auth::GoogleAccount,
            routes::auth::YouTubeChannel,
            routes::search::TopVideo,
            routes::search::SearchResult,
            routes::search::SearchSuggestions,
        )
    ),
    tags(
        (name = "YouTube Legacy API", description = "API server created to support YouTube clients for old devices")
    )
)]
struct ApiDoc;

#[derive(Debug, Deserialize, Serialize)]
struct AppState {
    config: Config,
}

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "API is running", body = String)
    )
)]
async fn health_check() -> impl Responder {
    log::info!("Health check endpoint called");
    HttpResponse::Ok().json("YouTube API Legacy is running!")
}

#[utoipa::path(
    get,
    path = "/",
    responses(
        (status = 200, description = "HTML page with API information")
    )
)]
async fn index(data: web::Data<AppState>) -> impl Responder {
    log::info!("Index page requested");
    let port = data.config.port;
    
    let html_content = stdfs::read_to_string("assets/html/index.html")
        .unwrap_or_else(|_| "Error loading HTML file".to_string());
    
    let html_content = html_content.replace("<!--PORT-->", &port.to_string());
    
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html_content)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    log::init_logger();
    
    check::perform_startup_checks();
    
    let config = Config::from_file("config.json")
        .expect("Failed to load config.json");
    
    let auth_config = AuthConfig {
        client_id: config.oauth_client_id.clone(),
        client_secret: config.oauth_client_secret.clone(),
        redirect_uri: format!("http://localhost:{}/oauth/callback", config.port),
        scopes: vec![
            "https://www.googleapis.com/auth/youtube.readonly".to_string(),
            "https://www.googleapis.com/auth/youtube".to_string(),
            "https://www.googleapis.com/auth/userinfo.profile".to_string(),
            "https://www.googleapis.com/auth/userinfo.email".to_string(),
        ],
    };
    
    let auth_config_data = web::Data::new(auth_config);
    let token_store_data = web::Data::new(TokenStore::new());
    
    let port = config.port;
    log::info!("Starting YouTube API Legacy server on port {}...", port);
    
    let app_state = web::Data::new(AppState { config });
    
    let openapi = ApiDoc::openapi();
    
    let server = HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .app_data(auth_config_data.clone())
            .app_data(token_store_data.clone())
            .wrap(Logger::default())
            .service(fs::Files::new("/assets", "assets/").show_files_listing())
            .service(
                SwaggerUi::new("/docs/{_:.*}")
                    .url("/openapi.json", openapi.clone())
            )
            .route("/", web::get().to(index))
            .route("/health", web::get().to(health_check))
            .route("/auth", web::get().to(routes::auth::auth_handler))
            .route("/auth/events", web::get().to(routes::auth::auth_events))
            .route("/oauth/callback", web::get().to(routes::auth::oauth_callback))
            .route("/account_info", web::get().to(routes::auth::account_info))
            .route("/get_top_videos.php", web::get().to(routes::search::get_top_videos))
            .route("/get_search_videos.php", web::get().to(routes::search::get_search_videos))
            .route("/get_search_suggestions.php", web::get().to(routes::search::get_search_suggestions))
            .route("/thumbnail/{video_id}", web::get().to(routes::video::thumbnail_proxy))
            .route("/channel_icon/{path_video_id}", web::get().to(routes::video::channel_icon))
    })
    .bind(("127.0.0.1", port))?
    .run();
    
    log::info!("Server running at http://127.0.0.1:{}/", port);
    log::info!("Documentation available at http://127.0.0.1:{}/docs", port);
    
    server.await
}