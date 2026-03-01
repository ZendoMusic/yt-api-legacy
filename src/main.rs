use actix_files as fs;
use actix_web::middleware::{NormalizePath, TrailingSlash};
use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use serde::{Deserialize, Serialize};
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
        routes::auth::auth_handler,
        routes::auth::auth_events,
        routes::auth::oauth_callback,
        routes::auth::account_info,
        routes::auth_routes::check_if_username_is_taken,
        routes::auth_routes::link_device_token,
        routes::auth_routes::get_session,
        routes::auth_routes::client_login,
        routes::auth_routes::youtube_client_login,
        routes::auth_routes::oauth2_token,
        routes::auth_routes::oauth2_userinfo,
        routes::search::get_top_videos,
        routes::search::get_search_videos,
        routes::search::get_search_suggestions,
        routes::search::get_categories,
        routes::search::get_categories_videos,
        routes::search::get_playlist_videos,
        routes::channel::get_author_videos,
        routes::channel::get_author_videos_by_id,
        routes::channel::get_channel_thumbnail_api,
        routes::video::get_ytvideo_info,
        routes::video::get_related_videos,
        routes::video::direct_url,
        routes::video::direct_audio_url,
        routes::video::get_direct_video_url,
        routes::video::hls_manifest_url,
        routes::video::video_proxy,
        routes::video::download_video,
        routes::additional::get_recommendations,
        routes::additional::get_subscriptions,
        routes::additional::get_history,
        routes::additional::mark_video_watched,
        routes::additional::get_instants,
        routes::additional::check_api_keys,
        routes::actions::subscribe,
        routes::actions::unsubscribe,
        routes::actions::rate,
        routes::actions::check_rating,
        routes::actions::check_subscription,
        routes::additional::check_failed_api_keys,
    ),
    components(
        schemas(
            Config,
            routes::auth::AccountInfoResponse,
            routes::auth::GoogleAccount,
            routes::auth::YouTubeChannel,
            routes::auth_routes::IsUsernameTakeResult,
            routes::auth_routes::OAuth2TokenResponse,
            routes::auth_routes::OAuth2UserInfoResponse,
            routes::search::TopVideo,
            routes::search::SearchResult,
            routes::search::CategoryItem,
            routes::search::PlaylistInfo,
            routes::search::PlaylistVideo,
            routes::search::PlaylistResponse,
            routes::channel::ChannelInfo,
            routes::channel::ChannelVideo,
            routes::channel::ChannelVideosResponse,
            routes::video::VideoInfoResponse,
            routes::video::Comment,
            routes::video::RelatedVideo,
            routes::video::DirectUrlResponse,
            routes::video::HlsManifestUrlResponse,
            routes::additional::RecommendationItem,
            routes::additional::HistoryItem,
            routes::additional::SubscriptionsResponse,
            routes::additional::InstantsResponse,
            routes::actions::YoutubeSubscriptionRequest,
            routes::actions::YoutubeRateRequest,
            routes::actions::YoutubeActionResponse,
            routes::actions::RatingCheckRequest,
            routes::actions::RatingCheckResponse,
            routes::actions::SubscriptionCheckRequest,
            routes::actions::SubscriptionCheckResponse,
            routes::additional::InstantItem,
        )
    ),
    tags(
        (name = "YouTube Legacy API", description = "API server created to support YouTube clients for old devices")
    )
)]
struct ApiDoc;

#[derive(Debug, Serialize)]
struct AppState {
    config: Config,
    /// Limits concurrent codec conversions (mpeg4/h263) for /direct_url.
    #[serde(skip)]
    codec_semaphore: std::sync::Arc<tokio::sync::Semaphore>,
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    log::init_logger();

    check::perform_startup_checks().await;

    let config = Config::from_file("config.yml").expect("Failed to load config.yml");

    let redirect_base = if let Some(custom) = config.api.oauth.redirect_uri.clone() {
        custom.trim_end_matches('/').to_string()
    } else if !config.server.main_url.is_empty() {
        config.server.main_url.trim_end_matches('/').to_string()
    } else if let Some(first) = config.instants.first() {
        first.0.trim_end_matches('/').to_string()
    } else {
        format!("http://localhost:{}", config.server.port)
    };

    let youtube_api_key = if !config.api.keys.active.is_empty() {
        config.api.keys.active[0].clone()
    } else {
        String::new()
    };

    let auth_config = AuthConfig {
        client_id: config.api.oauth.client_id.clone(),
        client_secret: config.api.oauth.client_secret.clone(),
        redirect_uri: if config.api.oauth.redirect_uri.is_some() {
            redirect_base.clone()
        } else {
            format!("{}/oauth/callback", redirect_base)
        },
        scopes: vec![
            "https://www.googleapis.com/auth/youtube.readonly".to_string(),
            "https://www.googleapis.com/auth/youtube".to_string(),
            "https://www.googleapis.com/auth/userinfo.profile".to_string(),
            "https://www.googleapis.com/auth/userinfo.email".to_string(),
        ],
        youtube_api_key,
    };

    let auth_config_data = web::Data::new(auth_config);
    let token_store_data = web::Data::new(TokenStore::new());

    let port = config.server.port;
    log::info!("Starting YouTube API Legacy server on port {}...", port);

    let codec_semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let app_state = web::Data::new(AppState {
        config,
        codec_semaphore,
    });

    let openapi = ApiDoc::openapi();

    let server = HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .app_data(auth_config_data.clone())
            .app_data(token_store_data.clone())
            .wrap(NormalizePath::new(TrailingSlash::MergeOnly))
            .wrap(log::SelectiveLogger::default())
            .service(fs::Files::new("/assets", "assets/").show_files_listing())
            .service(SwaggerUi::new("/docs/{_:.*}").url("/openapi.json", openapi.clone()))
            .route("/", web::get().to(routes::frontend::page_root))
            .route("/home", web::get().to(routes::frontend::page_index))
            .route("/results", web::get().to(routes::frontend::page_results))
            .route("/watch", web::get().to(routes::frontend::page_watch))
            .route("/channel", web::get().to(routes::frontend::page_channel))
            .route("/logout", web::get().to(routes::frontend::page_logout))
            .route("/embed/{video_id}", web::get().to(routes::frontend::page_embed))
            .route("/health", web::get().to(health_check))
            .route("/auth", web::get().to(routes::auth::auth_handler))
            .route("/auth/login", web::get().to(routes::frontend::page_login))
            .route("/auth/start", web::get().to(routes::auth::auth_start))
            .route("/auth/events", web::get().to(routes::auth::auth_events))
            .route(
                "/oauth/callback",
                web::get().to(routes::auth::oauth_callback),
            )
            .route("/account_info", web::get().to(routes::auth::account_info))
            .route(
                "/check_if_username_is_taken",
                web::get().to(routes::auth_routes::check_if_username_is_taken),
            )
            .route(
                "/link_device_token",
                web::post().to(routes::auth_routes::link_device_token),
            )
            .route(
                "/get_session",
                web::post().to(routes::auth_routes::get_session),
            )
            .route(
                "/accounts/ClientLogin",
                web::post().to(routes::auth_routes::client_login),
            )
            .route(
                "/youtube/accounts/ClientLogin",
                web::post().to(routes::auth_routes::youtube_client_login),
            )
            .route(
                "/o/oauth2/token",
                web::post().to(routes::auth_routes::oauth2_token),
            )
            .route(
                "/oauth2/v1/userinfo",
                web::get().to(routes::auth_routes::oauth2_userinfo),
            )
            .route(
                "/get_top_videos.php",
                web::get().to(routes::search::get_top_videos),
            )
            .route(
                "/get_search_videos.php",
                web::get().to(routes::search::get_search_videos),
            )
            .route(
                "/get_search_suggestions.php",
                web::get().to(routes::search::get_search_suggestions),
            )
            .route(
                "/get-categories.php",
                web::get().to(routes::search::get_categories),
            )
            .route(
                "/get-categories_videos.php",
                web::get().to(routes::search::get_categories_videos),
            )
            .route("/playlist", web::get().to(routes::search::playlist_root))
            .route(
                "/playlist/{playlist_id}",
                web::get().to(routes::search::get_playlist_videos),
            )
            .route(
                "/get_author_videos.php",
                web::get().to(routes::channel::get_author_videos),
            )
            .route(
                "/get_author_videos_by_id.php",
                web::get().to(routes::channel::get_author_videos_by_id),
            )
            .route(
                "/get_channel_thumbnail.php",
                web::get().to(routes::channel::get_channel_thumbnail_api),
            )
            .route(
                "/get-ytvideo-info.php",
                web::get().to(routes::video::get_ytvideo_info),
            )
            .route(
                "/get_related_videos.php",
                web::get().to(routes::video::get_related_videos),
            )
            .service(
                web::resource("/direct_url")
                    .route(web::get().to(routes::video::direct_url))
                    .route(web::head().to(routes::video::direct_url)),
            )
            .service(
                web::resource("/direct_audio_url")
                    .route(web::get().to(routes::video::direct_audio_url))
                    .route(web::head().to(routes::video::direct_audio_url)),
            )
            .service(
                web::resource("/hls_manifest_url")
                    .route(web::get().to(routes::video::hls_manifest_url)),
            )
            .route(
                "/get-direct-video-url.php",
                web::get().to(routes::video::get_direct_video_url),
            )
            .service(
                web::resource("/video.proxy")
                    .route(web::get().to(routes::video::video_proxy))
                    .route(web::head().to(routes::video::video_proxy)),
            )
            .route("/download", web::get().to(routes::video::download_video))
            .route(
                "/thumbnail/{video_id}",
                web::get().to(routes::video::thumbnail_proxy),
            )
            .route(
                "/channel_icon/{path_video_id}",
                web::get().to(routes::video::channel_icon),
            )
            .route(
                "/get_recommendations.php",
                web::get().to(routes::additional::get_recommendations),
            )
            .route(
                "/get_subscriptions.php",
                web::get().to(routes::additional::get_subscriptions),
            )
            .route(
                "/api/subscriptions_session",
                web::get().to(routes::additional::get_subscriptions_session),
            )
            .route(
                "/get_history.php",
                web::get().to(routes::additional::get_history),
            )
            .route(
                "/mark_video_watched.php",
                web::get().to(routes::additional::mark_video_watched),
            )
            .route(
                "/get-instants",
                web::get().to(routes::additional::get_instants),
            )
            .route(
                "/check_api_keys",
                web::get().to(routes::additional::check_api_keys),
            )
            .route(
                "/check_failed_api_keys",
                web::get().to(routes::additional::check_failed_api_keys),
            )
            .route(
                "/actions/subscribe",
                web::post().to(routes::actions::subscribe),
            )
            .route(
                "/actions/subscribe",
                web::get().to(routes::actions::subscribe),
            )
            .route(
                "/actions/unsubscribe",
                web::post().to(routes::actions::unsubscribe),
            )
            .route(
                "/actions/unsubscribe",
                web::get().to(routes::actions::unsubscribe),
            )
            .route("/actions/rate", web::post().to(routes::actions::rate))
            .route("/actions/rate", web::get().to(routes::actions::rate))
            .route(
                "/actions/check_rating",
                web::get().to(routes::actions::check_rating),
            )
            .route(
                "/actions/check_subscription",
                web::get().to(routes::actions::check_subscription),
            )
    })
    .bind(("0.0.0.0", port))?
    .run();

    log::info!("Server running at http://127.0.0.1:{}/", port);

    server.await
}
