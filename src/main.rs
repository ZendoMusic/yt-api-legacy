use actix_web::{web, App, HttpResponse, HttpServer, Responder, middleware::Logger};
use serde::{Deserialize, Serialize};

mod config;
use config::Config;

#[derive(Debug, Deserialize, Serialize)]
struct AppState {
    config: Config,
}

async fn health_check() -> impl Responder {
    log::info!("Health check endpoint called");
    HttpResponse::Ok().json("YouTube API Legacy is running!")
}

async fn index(data: web::Data<AppState>) -> impl Responder {
    log::info!("Index page requested");
    let port = data.config.port;
    let html = format!(r#"
    <!DOCTYPE html>
    <html lang="en">
    <head>
        <meta charset="UTF-8">
        <meta name="viewport" content="width=device-width, initial-scale=1.0">
        <title>YouTube Legacy API</title>
        <style>
            body {{
                margin: 0;
                padding: 0;
                font-family: 'Segoe UI', sans-serif;
                background: #1a1a1a;
                color: #fff;
                display: flex;
                flex-direction: column;
                align-items: center;
                justify-content: center;
                min-height: 100vh;
            }}
            .container {{
                text-align: center;
                padding: 20px;
                max-width: 800px;
            }}
            h1 {{
                font-size: 2.5em;
                margin: 0;
                color: #fff;
            }}
            .subtitle {{
                font-size: 1.2em;
                color: #888;
                margin: 10px 0 30px;
            }}
            .tile {{
                background: #2d2d2d;
                border-radius: 10px;
                padding: 20px;
                margin: 10px 0;
                text-align: left;
            }}
            .tile h2 {{
                margin: 0 0 10px;
                color: #fff;
            }}
            .tile p {{
                margin: 0;
                color: #888;
            }}
            .footer {{
                margin-top: 40px;
                color: #666;
                font-size: 0.9em;
            }}
        </style>
    </head>
    <body>
        <div class="container">
            <h1>YouTube Legacy API</h1>
            <div class="subtitle">A Windows Phone inspired YouTube API service</div>
            <div class="tile">
                <h2>Status</h2>
                <p>API is running on port {}</p>
            </div>
            <div class="footer">
                LegacyProjects YouTube API Service
            </div>
        </div>
    </body>
    </html>
    "#, port);
    
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    std::env::set_var("RUST_LOG", "info");
    env_logger::builder()
        .format_timestamp_secs()
        .format_module_path(false)
        .format_target(false)
        .filter_level(log::LevelFilter::Info)
        .init();
    
    let config = Config::from_file("config.json")
        .expect("Failed to load config.json");
    
    let port = config.port;
    log::info!("Starting YouTube API Legacy server on port {}...", port);
    
    let app_state = web::Data::new(AppState { config });
    
    let server = HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .wrap(Logger::default())
            .route("/", web::get().to(index))
            .route("/health", web::get().to(health_check))
    })
    .bind(("127.0.0.1", port))?
    .run();
    
    log::info!("Server running at http://127.0.0.1:{}/", port);
    
    server.await
}