//! Frontend: serves HTML pages with data substituted by Rust (yt2014-style).
//! Templates are in assets/html/frontend/, assets at /assets.

use actix_web::{web, HttpRequest, HttpResponse, Responder};
use html_escape::encode_text;
use serde::Deserialize;
use std::fs;

use crate::config::Config;
use crate::routes::additional::{HistoryItem, RecommendationItem};
use crate::routes::auth::{AuthConfig, TokenStore};
use crate::routes::channel::{ChannelVideosResponse, ChannelVideo};
use crate::routes::search::{SearchResult, TopVideo};
use crate::routes::video::{RelatedVideo, VideoInfoResponse};

fn base_url(req: &HttpRequest, config: &Config) -> String {
    if !config.server.main_url.is_empty() {
        return config.server.main_url.trim_end_matches('/').to_string();
    }
    let info = req.connection_info();
    let scheme = info.scheme();
    let host = info.host();
    format!("{}://{}", scheme, host.trim_end_matches('/'))
}

fn load_template(name: &str) -> String {
    let path = format!("assets/html/frontend/{}.html", name);
    fs::read_to_string(&path).unwrap_or_else(|_| format!("<!-- template {} not found -->", name))
}

fn load_root_index() -> String {
    fs::read_to_string("assets/html/index.html")
        .unwrap_or_else(|_| "<!-- assets/html/index.html not found -->".to_string())
}

async fn fetch_json<T: for<'de> Deserialize<'de>>(
    base: &str,
    path: &str,
) -> Result<T, String> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("API returned {}", resp.status()));
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

fn h(s: &str) -> String {
    encode_text(s).to_string()
}

fn make_clickable(text: &str) -> String {
    // Simple: escape HTML and turn URLs into links, newlines to <br>
    let escaped = h(text);
    let with_br = escaped.replace("\n", "<br>");
    // Very simple URL detection
    let url_regex = regex::Regex::new(r"https?://[^\s<>]+").unwrap();
    url_regex
        .replace_all(&with_br, |caps: &regex::Captures| {
            let u = &caps[0];
            format!(r#"<a href="{}" target="_blank" rel="noopener">{}</a>"#, u, u)
        })
        .to_string()
}

// ---- Navbar (included in every page) ----
fn render_navbar(main_url: &str, search_query: &str) -> String {
    let t = load_template("partials/navbar");
    t.replace("{{MAIN_URL}}", main_url)
        .replace("{{SEARCH_QUERY}}", &h(search_query))
}

// ---- Sidebar (guide) - separate partial; tech section only on root page
fn render_sidebar(main_url: &str, tech_section: Option<&str>) -> String {
    let t = load_template("partials/sidebar");
    let t = t.replace("{{MAIN_URL}}", main_url);
    t.replace("{{SIDEBAR_TECH_SECTION}}", tech_section.unwrap_or(""))
}

fn render_sidebar_tech_section(port: u16, instants: &[crate::config::InstantInstance], main_url: &str) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "<p class=\"guide-tech-line\"><strong>Port:</strong> {}</p>",
        port
    ));
    body.push_str("<p class=\"guide-tech-line\"><strong>Instances</strong></p><ul class=\"guide-tech-list\">");
    if instants.is_empty() {
        body.push_str("<li>None configured</li>");
    } else {
        for inst in instants {
            let url = h(&inst.0);
            let link = format!("{}/", inst.0.trim_end_matches('/'));
            body.push_str(&format!(
                "<li><a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a></li>",
                h(&link),
                url
            ));
        }
    }
    body.push_str("</ul>");
    body.push_str(&format!(
        "<p class=\"guide-tech-line\"><a href=\"{}/docs/\">Documentation</a></p>",
        main_url
    ));
    body.push_str(
        "<p class=\"guide-tech-line\"><a href=\"https://github.com/ZendoMusic/yt-api-legacy\" target=\"_blank\" rel=\"noopener\">GitHub</a></p>",
    );
    format!(
        r#"<li class="guide-section vve-check guide-section-service">
            <div class="guide-item-container personal-item">
              <h3>Service</h3>
              <div class="guide-service-tech">{}</div>
            </div>
            <hr class="guide-section-separator">
          </li>"#,
        body
    )
}

// ---- Root "/": index with navbar, sidebar, videos, recommendations shelf, tech footer ----
pub async fn page_root(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    auth_config: web::Data<AuthConfig>,
    token_store: web::Data<TokenStore>,
) -> impl Responder {
    let config = &data.config;
    let main_url = base_url(&req, config);
    let main_url_trimmed = main_url.trim_end_matches('/');
    let port = config.server.port;

    let videos: Vec<TopVideo> = match fetch_json::<Vec<TopVideo>>(
        &main_url,
        "/get_top_videos.php?count=24",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            crate::log::info!("Root index: failed to fetch top videos: {}", e);
            Vec::new()
        }
    };

    let refresh_token = req
        .cookie("session_id")
        .and_then(|c| token_store.get_token(c.value()))
        .filter(|t| !t.is_empty() && !t.starts_with("Error"));

    let recommendations = match refresh_token {
        Some(ref token) => crate::routes::additional::fetch_recommendations_for_token(
            token,
            &auth_config,
            config,
            main_url_trimmed,
            24,
        )
        .await
        .unwrap_or_default(),
        None => Vec::new(),
    };

    let history = match refresh_token {
        Some(ref token) => {
            crate::routes::additional::fetch_history_for_token(
                token,
                &auth_config,
                config,
                main_url_trimmed,
                24,
            )
            .await
        }
        None => Vec::new(),
    };

    let navbar = render_navbar(&main_url, "");
    let sidebar_tech_section = render_sidebar_tech_section(port, &config.instants, &main_url);
    let sidebar_html = render_sidebar(&main_url, Some(&sidebar_tech_section));
    let (main_content, subscriptions_sidebar, body_class) = match refresh_token {
        Some(_) => {
            let videos_grid = render_video_grid(&videos, &main_url);
            let recommendations_shelf = render_recommendations_shelf(&recommendations, &main_url);
            let history_shelf = render_history_shelf(&history, &main_url);
            let content = format!(
                r#"<div class="compact-shelf-content-container">
                      <div class="yt-uix-shelfslider-body">
                        <ul class="yt-uix-shelfslider-list">{}</ul>
                      </div>
                    </div>
                    {}
                    {}"#,
                videos_grid,
                recommendations_shelf,
                history_shelf
            );
            (content, subscriptions_sidebar_loading_placeholder(), String::new())
        }
        None => (
            logged_out_main_placeholder(),
            String::new(),
            "home-logged-out".to_string(),
        ),
    };

    let t = load_root_index();
    let html = t
        .replace("{{NAVBAR}}", &navbar)
        .replace("{{SIDEBAR}}", &sidebar_html)
        .replace("{{MAIN_URL}}", &main_url)
        .replace("{{PORT}}", &port.to_string())
        .replace("{{MAIN_CONTENT}}", &main_content)
        .replace("{{SUBSCRIPTIONS_SIDEBAR}}", &subscriptions_sidebar)
        .replace("{{BODY_CLASS}}", &body_class);

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

fn thumb_url(v: &RecommendationItem, base: &str) -> String {
    if v.thumbnail.is_empty() {
        format!("{}/thumbnail/{}", base.trim_end_matches('/'), v.video_id)
    } else {
        v.thumbnail.clone()
    }
}

fn render_recommendations_shelf(items: &[RecommendationItem], main_url: &str) -> String {
    if items.is_empty() {
        return String::new();
    }
    let base = main_url.trim_end_matches('/');
    let watch_url = |vid: &str| format!("{}/watch?v={}", main_url, vid);

    let mut featured = String::new();
    if let Some(large) = items.first() {
        let thumb = thumb_url(large, base);
        let w = watch_url(&large.video_id);
        featured.push_str(&format!(
            r#"<br>
<div class="shelf-wrapper clearfix">
  <div class="lohp-newspaper-shelf shelf-item vve-check  yt-section-hover-container">
    <div class="lohp-shelf-content">
      <div class="lohp-large-shelf-container">
        <div class="clearfix">
          <div class="vve-check">
            <a href="{}" class="ux-thumb-wrap yt-uix-sessionlink yt-fluid-thumb-link contains-addto lohp-thumb-wrap spf-link">
              <span class="video-thumb  yt-thumb yt-thumb-370 yt-thumb-fluid">
                <span class="yt-thumb-default">
                  <span class="yt-thumb-clip">
                    <img src="{}" alt="Thumbnail" width="370">
                    <span class="vertical-align"></span>
                  </span>
                </span>
              </span>
              <span class="video-time">{}</span>
            </a>
            <a class="lohp-video-link max-line-2 yt-uix-sessionlink spf-link" href="{}" title="{}">{}</a>
          </div>
        </div>
        <div class="lohp-video-metadata">
          <span class="content-uploader lohp-video-metadata-item spf-link">
            <span class="username-prepend">by</span> <a href="{}" class="g-hovercard yt-uix-sessionlink yt-user-name spf-link">{}</a>
          </span>
        </div>
      </div>
      <div class="lohp-medium-shelves-container">"#,
            h(&w),
            h(&thumb),
            h(&large.duration),
            h(&w),
            h(&large.title),
            h(&large.title),
            format!("{}/results?search_query={}", main_url, urlencoding::encode(&large.author)),
            h(&large.author)
        ));
        for (_, v) in items.iter().skip(1).take(3).enumerate() {
            let thumb = thumb_url(v, base);
            let w = watch_url(&v.video_id);
            featured.push_str(&format!(
                r#"<div class="lohp-medium-shelf vve-check spf-link">
        <div class="vve-check">
          <div class="lohp-media-object">
            <a href="{}" class="ux-thumb-wrap yt-uix-sessionlink yt-fluid-thumb-link contains-addto lohp-thumb-wrap">
              <span class="video-thumb  yt-thumb yt-thumb-170 yt-thumb-fluid">
                <span class="yt-thumb-default">
                  <span class="yt-thumb-clip">
                    <img src="{}" alt="Thumbnail" width="170">
                    <span class="vertical-align"></span>
                  </span>
                </span>
              </span>
              <span class="video-time">{}</span>
            </a>
          </div>
          <div class="lohp-media-object-content lohp-medium-shelf-content">
            <a class="lohp-video-link max-line-2 yt-uix-sessionlink spf-link" href="{}" title="{}">{}</a>
            <div class="lohp-video-metadata attached">
              <span class="content-uploader  spf-link">
                <span class="username-prepend">by</span> <a href="{}" class="g-hovercard yt-uix-sessionlink yt-user-name spf-link">{}</a>
              </span>
            </div>
          </div>
        </div>
      </div>"#,
                h(&w),
                h(&thumb),
                h(&v.duration),
                h(&w),
                h(&v.title),
                h(&v.title),
                format!("{}/results?search_query={}", main_url, urlencoding::encode(&v.author)),
                h(&v.author)
            ));
        }
        featured.push_str("</div></div></div></div>");
    }

    let mut list = String::new();
    for v in items.iter().skip(4) {
        let w = watch_url(&v.video_id);
        let thumb = thumb_url(v, base);
        let author_url = format!("{}/results?search_query={}", main_url, urlencoding::encode(&v.author));
        list.push_str(&format!(
            r#"<li class="channels-content-item yt-shelf-grid-item yt-uix-shelfslider-item ">
    <div class="yt-lockup clearfix  yt-lockup-video yt-lockup-grid vve-check">
    <div class="yt-lockup-thumbnail">
      <a href="{}" class="ux-thumb-wrap yt-uix-sessionlink yt-fluid-thumb-link contains-addto spf-link">
        <span class="video-thumb  yt-thumb yt-thumb-175 yt-thumb-fluid">
          <span class="yt-thumb-default">
            <span class="yt-thumb-clip">
              <img src="{}" alt="Thumbnail" width="175">
              <span class="vertical-align"></span>
            </span>
          </span>
        </span>
        <span class="video-time">{}</span>
      </a>
    </div>
    <div class="yt-lockup-content">
      <h3 class="yt-lockup-title"><a class="yt-uix-sessionlink yt-uix-tile-link spf-link yt-ui-ellipsis yt-ui-ellipsis-2" href="{}" title="{}">{}</a></h3>
      <div class="yt-lockup-meta">
        <ul class="yt-lockup-meta-info">
          <li>by <a href="{}" class="g-hovercard yt-uix-sessionlink yt-user-name spf-link">{}</a></li>
        </ul>
      </div>
    </div>
  </div>
</li>"#,
            h(&w),
            h(&thumb),
            h(&v.duration),
            h(&w),
            h(&v.title),
            h(&v.title),
            h(&author_url),
            h(&v.author)
        ));
    }

    let rest_shelf = if list.is_empty() {
        String::new()
    } else {
        format!(
            r#"<br>
<div class="shelf-wrapper clearfix">
  <div class="compact-shelf shelf-item yt-uix-shelfslider clearfix">
    <h2 class="branded-page-module-title">Recommended</h2>
    <div class="compact-shelf-content-container">
      <div class="yt-uix-shelfslider-body">
        <ul class="yt-uix-shelfslider-list">{}</ul>
      </div>
    </div>
  </div>
</div>"#,
            list
        )
    };
    format!("{}{}", featured, rest_shelf)
}

fn history_thumb_url(v: &HistoryItem, base: &str) -> String {
    if v.thumbnail.is_empty() {
        format!("{}/thumbnail/{}", base.trim_end_matches('/'), v.video_id)
    } else {
        v.thumbnail.clone()
    }
}

fn render_history_shelf(items: &[HistoryItem], main_url: &str) -> String {
    if items.is_empty() {
        return String::new();
    }
    let base = main_url.trim_end_matches('/');
    let watch_url = |vid: &str| format!("{}/watch?v={}", main_url, vid);
    let mut list = String::new();
    for v in items {
        let w = watch_url(&v.video_id);
        let thumb = history_thumb_url(v, base);
        let author_url = format!(
            "{}/results?search_query={}",
            main_url,
            urlencoding::encode(&v.author)
        );
        list.push_str(&format!(
            r#"<li class="channels-content-item yt-shelf-grid-item yt-uix-shelfslider-item ">
    <div class="yt-lockup clearfix  yt-lockup-video yt-lockup-grid vve-check">
    <div class="yt-lockup-thumbnail">
      <a href="{}" class="ux-thumb-wrap yt-uix-sessionlink yt-fluid-thumb-link contains-addto spf-link">
        <span class="video-thumb  yt-thumb yt-thumb-175 yt-thumb-fluid">
          <span class="yt-thumb-default">
            <span class="yt-thumb-clip">
              <img src="{}" alt="Thumbnail" width="175">
              <span class="vertical-align"></span>
            </span>
          </span>
        </span>
        <span class="video-time">{}</span>
      </a>
    </div>
    <div class="yt-lockup-content">
      <h3 class="yt-lockup-title"><a class="yt-uix-sessionlink yt-uix-tile-link spf-link yt-ui-ellipsis yt-ui-ellipsis-2" href="{}" title="{}">{}</a></h3>
      <div class="yt-lockup-meta">
        <ul class="yt-lockup-meta-info">
          <li>by <a href="{}" class="g-hovercard yt-uix-sessionlink yt-user-name spf-link">{}</a></li>
        </ul>
      </div>
    </div>
  </div>
</li>"#,
            h(&w),
            h(&thumb),
            h(&v.duration),
            h(&w),
            h(&v.title),
            h(&v.title),
            h(&author_url),
            h(&v.author)
        ));
    }
    format!(
        r#"<br>
<div class="shelf-wrapper clearfix">
  <div class="compact-shelf shelf-item yt-uix-shelfslider clearfix">
    <h2 class="branded-page-module-title">Watch history</h2>
    <div class="compact-shelf-content-container">
      <div class="yt-uix-shelfslider-body">
        <ul class="yt-uix-shelfslider-list">{}</ul>
      </div>
    </div>
  </div>
</div>"#,
        list
    )
}

/// When not logged in: square box with gray border and "Try to find something" instead of videos/recommendations/history.
fn logged_out_main_placeholder() -> String {
    r#"<div class="home-logged-out-placeholder">
  <p class="home-logged-out-text">Try to find something</p>
</div>"#
        .to_string()
}

/// Placeholder for subscriptions sidebar: same loading GIF + "Loading..." as on login (QR load). JS replaces #subscriptions-sidebar-content with the list.
fn subscriptions_sidebar_loading_placeholder() -> String {
    r#"<div class="branded-page-related-channels branded-page-box yt-card">
  <h2 class="branded-page-module-title" dir="ltr">Subscriptions</h2>
  <div id="subscriptions-sidebar-content" class="subscriptions-loading-box">
    <p class="yt-spinner">
      <img class="yt-spinner-img" src="/assets/images/pixel-vfl3z5WfW.gif" alt="Loading" title="">
    </p>
    <span class="yt-spinner-message">Loading...</span>
  </div>
</div>"#
        .to_string()
}

// ---- Home (index): top videos — same structure as yt2014 index (compact shelf) ----
fn render_video_grid(videos: &[TopVideo], main_url: &str) -> String {
    let base = main_url.trim_end_matches('/');
    let mut out = String::new();
    for v in videos {
        let watch_url = format!("{}/watch?v={}", main_url, v.video_id);
        let thumb = if v.thumbnail.is_empty() {
            format!("{}/thumbnail/{}", base, v.video_id)
        } else {
            v.thumbnail.clone()
        };
        let author_url = format!("{}/results?search_query={}", main_url, urlencoding::encode(&v.author));
        out.push_str(&format!(
            r#"<li class="channels-content-item yt-shelf-grid-item yt-uix-shelfslider-item ">
    <div class="yt-lockup clearfix  yt-lockup-video yt-lockup-grid vve-check">
    <div class="yt-lockup-thumbnail">
      <a href="{}" class="ux-thumb-wrap yt-uix-sessionlink yt-fluid-thumb-link contains-addto spf-link">
        <span class="video-thumb  yt-thumb yt-thumb-175 yt-thumb-fluid">
          <span class="yt-thumb-default">
            <span class="yt-thumb-clip">
              <img src="{}" alt="Thumbnail" width="175">
              <span class="vertical-align"></span>
            </span>
          </span>
        </span>
        <span class="video-time">{}</span>
      </a>
    </div>
    <div class="yt-lockup-content">
      <h3 class="yt-lockup-title"><a class="yt-uix-sessionlink yt-uix-tile-link spf-link yt-ui-ellipsis yt-ui-ellipsis-2" href="{}" title="{}">{}</a></h3>
      <div class="yt-lockup-meta">
        <ul class="yt-lockup-meta-info">
          <li>by <a href="{}" class="g-hovercard yt-uix-sessionlink yt-user-name spf-link">{}</a></li>
        </ul>
      </div>
    </div>
  </div>
</li>"#,
            h(&watch_url),
            h(&thumb),
            h(&v.duration),
            h(&watch_url),
            h(&v.title),
            h(&v.title),
            h(&author_url),
            h(&v.author)
        ));
    }
    out
}

pub async fn page_index(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let main_url = base.clone();

    let videos: Vec<TopVideo> = match fetch_json::<Vec<TopVideo>>(
        &base,
        "/get_top_videos.php?count=24",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            crate::log::info!("Frontend index: failed to fetch top videos: {}", e);
            Vec::new()
        }
    };

    let navbar = render_navbar(&main_url, "");
    let videos_grid = render_video_grid(&videos, &main_url);

    let t = load_template("index");
    let html = t
        .replace("{{NAVBAR}}", &navbar)
        .replace("{{MAIN_URL}}", &main_url)
        .replace("{{VIDEOS_GRID}}", &videos_grid);

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

// ---- Results: search ----
fn render_search_results(videos: &[SearchResult], main_url: &str) -> String {
    let mut out = String::new();
    for v in videos {
        let video_id = v.video_id.as_deref().unwrap_or("");
        if video_id.is_empty() {
            continue;
        }
        let watch_url = format!("{}/watch?v={}", main_url, h(video_id));
        out.push_str(&format!(
            r#"<li class="yt-lockup clearfix yt-lockup-video yt-lockup-tile result-item-padding">
    <div class="yt-lockup-thumbnail">
        <a href="{}" class="ux-thumb-wrap spf-link">
            <span class="video-thumb yt-thumb yt-thumb-185">
                <span class="yt-thumb-default">
                    <span class="yt-thumb-clip">
                        <img alt="{}" src="{}" width="185" height="104">
                        <span class="vertical-align"></span>
                    </span>
                </span>
            </span>
            <span class="video-time">{}</span>
        </a>
    </div>
    <div class="yt-lockup-content">
        <h3 class="yt-lockup-title">
            <a class="yt-uix-tile-link spf-link yt-ui-ellipsis-2" href="{}" title="{}">{}</a>
        </h3>
        <div class="yt-lockup-meta"><ul class="yt-lockup-meta-info"><li>{}</li></ul></div>
    </div>
</li>"#,
            watch_url,
            h(&v.title),
            v.thumbnail,
            v.duration.as_deref().unwrap_or(""),
            watch_url,
            h(&v.title),
            h(&v.title),
            h(&v.author)
        ));
    }
    out
}

#[derive(serde::Deserialize)]
pub struct ResultsQuery {
    search_query: Option<String>,
}

pub async fn page_results(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    query: web::Query<ResultsQuery>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let main_url = base.clone();
    let search_query = query
        .search_query
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let search_encoded = urlencoding::encode(&search_query);

    let videos: Vec<SearchResult> = if search_query.is_empty() {
        Vec::new()
    } else {
        match fetch_json::<Vec<SearchResult>>(
            &base,
            &format!("/get_search_videos.php?query={}", search_encoded),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                crate::log::info!("Frontend results: failed to fetch search: {}", e);
                Vec::new()
            }
        }
    };

    let navbar = render_navbar(&main_url, &search_query);
    let sidebar_html = render_sidebar(&main_url, None);
    let results_html = if videos.is_empty() && !search_query.is_empty() {
        format!(
            r#"<div class="yt-alert yt-alert-default"><div class="yt-alert-content">No results for "{}"</div></div>"#,
            h(&search_query)
        )
    } else {
        render_search_results(&videos, &main_url)
    };

    let t = load_template("results");
    let html = t
        .replace("{{NAVBAR}}", &navbar)
        .replace("{{SIDEBAR}}", &sidebar_html)
        .replace("{{MAIN_URL}}", &main_url)
        .replace("{{SEARCH_QUERY}}", &h(&search_query))
        .replace("{{RESULTS}}", &results_html);

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

// ---- Watch: single video ----
fn render_related_list(videos: &[RelatedVideo], main_url: &str) -> String {
    let mut out = String::new();
    for v in videos {
        let watch_url = format!("{}/watch?v={}", main_url, h(&v.video_id));
        let thumb = if v.thumbnail.is_empty() {
            "/assets/images/mqdefault.webp".to_string()
        } else if v.thumbnail.contains('?') {
            format!("{}&quality=default", v.thumbnail)
        } else {
            format!("{}?quality=default", v.thumbnail)
        };
        out.push_str(&format!(
            r#"<li class="video-list-item related-list-item">
    <a href="{}" class="related-video spf-link yt-uix-sessionlink" data-sessionlink="feature=relmfu">
        <span class="yt-uix-simple-thumb-wrap yt-uix-simple-thumb-related" data-vid="{}">
            <img alt="{}" src="{}" width="120" height="90">
        </span>
        <span dir="ltr" class="title" title="{}">{}</span>
        <span class="stat attribution">{}</span>
        <span class="stat view-count">{}</span>
    </a>
</li>"#,
            watch_url,
            h(&v.video_id),
            h(&v.title),
            thumb,
            h(&v.title),
            h(&v.title),
            h(&v.author),
            v.views
        ));
    }
    out
}

fn render_comments(comments: &[crate::routes::video::Comment], main_url: &str) -> String {
    let mut out = String::new();
    for c in comments.iter().take(20) {
        let author = c.author.as_str();
        let text = c.text.as_str();
        let published = c.published_at.as_str();
        let thumb = if c.author_thumbnail.is_empty() {
            "/assets/images/photo.jpg"
        } else {
            c.author_thumbnail.as_str()
        };
        let channel_link = format!(
            "{}/channel?handle={}",
            main_url,
            urlencoding::encode(author)
        );
        out.push_str(&format!(
            r#"<div class="comment-item clearfix">
    <a href="{}" class="comment-author-thumb-link"><div class="comment-author-thumb">
        <img src="{}" alt="{}" width="48" height="48">
    </div></a>
    <div class="comment-body">
        <div class="comment-header">
            <a href="{}" class="comment-author">{}</a>
            <span class="comment-time">{}</span>
        </div>
        <div class="comment-text">{}</div>
    </div>
</div>"#,
            channel_link,
            thumb,
            h(author),
            channel_link,
            h(author),
            h(published),
            make_clickable(text)
        ));
    }
    out
}

#[derive(serde::Deserialize)]
pub struct WatchQuery {
    v: Option<String>,
}

pub async fn page_watch(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    query: web::Query<WatchQuery>,
) -> impl Responder {
    let video_id = match &query.v {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            return HttpResponse::BadRequest()
                .content_type("text/html; charset=utf-8")
                .body("<h1>Missing video ID</h1><p>Use ?v=VIDEO_ID</p>");
        }
    };

    let config = &data.config;
    let base = base_url(&req, config);
    let main_url = base.clone();
    let base_trimmed = main_url.trim_end_matches('/');

    let info: VideoInfoResponse = match fetch_json(
        &base,
        &format!("/get-ytvideo-info.php?video_id={}", urlencoding::encode(&video_id)),
    )
    .await
    {
        Ok(i) => i,
        Err(e) => {
            crate::log::info!("Frontend watch: failed to fetch video info: {}", e);
            return HttpResponse::InternalServerError()
                .content_type("text/html; charset=utf-8")
                .body(format!("<h1>Video not found</h1><p>{}</p>", h(&e)));
        }
    };

    let related: Vec<RelatedVideo> = fetch_json(
        &base,
        &format!("/get_related_videos.php?video_id={}", urlencoding::encode(&video_id)),
    )
    .await
    .unwrap_or_default();

    let title = info.title.as_str();
    let channel_url = info
        .channel_custom_url
        .as_deref()
        .unwrap_or("");
    let channel_link = if channel_url.is_empty() {
        String::new()
    } else {
        format!("{}/channel?handle={}", main_url, urlencoding::encode(channel_url))
    };
    let author = info.author.as_str();
    let channel_thumb = if info.channel_thumbnail.is_empty() {
        "/assets/images/photo.jpg"
    } else {
        &info.channel_thumbnail
    };
    let views = info.views.as_deref().unwrap_or("0");
    let subscriber_count = info.subscriber_count.as_str();
    let likes = info.likes.as_deref().unwrap_or("0");
    let published_at = info.published_at.as_str();
    let description = info.description.as_str();
    let comment_count = info.comment_count.as_deref().unwrap_or("0");
    let comments = &info.comments;

    let video_src = if base_trimmed.is_empty() {
        format!("/direct_url?video_id={}", urlencoding::encode(&video_id))
    } else {
        format!(
            "{}/direct_url?video_id={}",
            base_trimmed,
            urlencoding::encode(&video_id)
        )
    };
    let poster = if base_trimmed.is_empty() {
        format!("/thumbnail/{}", urlencoding::encode(&video_id))
    } else {
        format!("{}/thumbnail/{}", base_trimmed, urlencoding::encode(&video_id))
    };

    let navbar = render_navbar(&main_url, "");
    let related_html = if related.is_empty() {
        "<li style='padding:20px;color:#aaa'>No related videos</li>".to_string()
    } else {
        render_related_list(&related, &main_url)
    };
    let comments_html = if comments.is_empty() {
        "<div class='comment-empty'><p>No comments yet.</p></div>".to_string()
    } else {
        render_comments(comments, &main_url)
    };

    let t = load_template("watch");
    let html = t
        .replace("{{NAVBAR}}", &navbar)
        .replace("{{MAIN_URL}}", &main_url)
        .replace("{{VIDEO_ID}}", &h(&video_id))
        .replace("{{PAGE_TITLE}}", &format!("{} - YouTube", h(title)))
        .replace("{{VIDEO_TITLE}}", &h(title))
        .replace("{{CHANNEL_LINK}}", &channel_link)
        .replace("{{CHANNEL_THUMB}}", channel_thumb)
        .replace("{{AUTHOR}}", &h(author))
        .replace("{{SUBSCRIBER_COUNT}}", subscriber_count)
        .replace("{{VIEWS}}", views)
        .replace("{{LIKE_RATIO}}", "50")
        .replace("{{DISLIKE_RATIO}}", "50")
        .replace("{{LIKES}}", likes)
        .replace("{{PUBLISHED_AT}}", &h(published_at))
        .replace("{{DESCRIPTION_HTML}}", &make_clickable(description))
        .replace("{{COMMENT_COUNT}}", comment_count)
        .replace("{{COMMENTS_HTML}}", &comments_html)
        .replace("{{RELATED_VIDEOS}}", &related_html)
        .replace("{{VIDEO_SRC}}", &h(&video_src))
        .replace("{{POSTER}}", &h(&poster));

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

// ---- Channel ----
/// Parse views string (e.g. "1,234" or "1234") to number for comparison.
fn parse_views(views: &str) -> u64 {
    views
        .replace(',', "")
        .replace(' ', "")
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .fold(0u64, |n, c| n * 10 + (c as u64 - b'0' as u64))
}

/// Render spotlight block (most viewed video) like yt2014, or empty-state block.
fn render_spotlight_html(videos: &[ChannelVideo], main_url: &str) -> String {
    let spotlight = videos
        .iter()
        .max_by_key(|v| parse_views(&v.views))
        .filter(|v| !v.video_id.is_empty());

    if let Some(v) = spotlight {
        let embed_src = format!("{}/embed/{}", main_url, h(&v.video_id));
        let watch_url = format!("{}/watch?v={}", main_url, h(&v.video_id));
        format!(
            r#"<div class="c4-spotlight-module yt-section-hover-container">
      <div class="c4-spotlight-module-component upsell">
  <div class="upsell-video-container yt-section-hover-container">
          <div class="video-player-view-component branded-page-box">
    <div class="video-content clearfix ">
        <div class="c4-player-container c4-flexible-player-container">
      <div class="c4-flexible-height-setter"></div>
      <div id="upsell-video" class="c4-flexible-player-box">
        <iframe width="100%" height="100%" src="{}" allowfullscreen></iframe>
      </div>
  </div>
        <div class="video-detail">
      <h3 class="title">
        <a href="{}" class="yt-uix-sessionlink yt-ui-ellipsis yt-ui-ellipsis-2 spf-link">{}</a>
      </h3>
      <div class="view-count">
        <span class="count">{} views</span>
        <span class="content-item-time-created">{}</span>
      </div>
  </div>
      <div class="video-content-info"></div>
    </div>
  </div>
  </div>
      </div>"#,
            embed_src,
            watch_url,
            h(&v.title),
            h(&v.views),
            h(&v.published_at)
        )
    } else {
        r#"<div class="c4-spotlight-module yt-section-hover-container">
      <div class="c4-spotlight-module-component upsell">
  <div class="upsell-video-container yt-section-hover-container">
          <div class="video-player-view-component branded-page-box">
    <div class="video-content clearfix ">
        <div class="c4-player-container c4-flexible-player-container">
      <div class="c4-flexible-height-setter"></div>
      <div id="upsell-video" class="c4-flexible-player-box">
        <p>No spotlight video available</p>
      </div>
  </div>
        <div class="video-detail">
      <h3 class="title"><span>No videos found</span></h3>
      <div class="view-count"><span class="count">0 views</span></div>
    </div>
      <div class="video-content-info"></div>
    </div>
  </div>
  </div>
      </div>"#.to_string()
    }
}

const VIDEOS_PER_ROW: usize = 6;

fn render_channel_video_item(v: &ChannelVideo, main_url: &str) -> String {
    let watch_url = format!("{}/watch?v={}", main_url, h(&v.video_id));
    let thumb = if v.thumbnail.is_empty() {
        "/assets/images/mqdefault.webp"
    } else {
        v.thumbnail.as_str()
    };
    format!(
        r#"<li class="channels-content-item yt-shelf-grid-item yt-uix-shelfslider-item">
    <div class="yt-lockup clearfix yt-lockup-video yt-lockup-grid">
        <div class="yt-lockup-thumbnail">
            <a href="{}" class="ux-thumb-wrap yt-uix-sessionlink yt-fluid-thumb-link contains-addto spf-link">
                <span class="video-thumb yt-thumb yt-thumb-185 yt-thumb-fluid">
                    <span class="yt-thumb-default">
                        <span class="yt-thumb-clip">
                            <img src="{}" alt="{}" width="185">
                            <span class="vertical-align"></span>
                        </span>
                    </span>
                </span>
                <span class="video-time">{}</span>
            </a>
        </div>
        <div class="yt-lockup-content">
            <h3 class="yt-lockup-title"><a href="{}" class="yt-uix-sessionlink yt-uix-tile-link spf-link yt-ui-ellipsis yt-ui-ellipsis-2" dir="ltr" title="{}">{}</a></h3>
            <div class="yt-lockup-meta">
                <ul class="yt-lockup-meta-info">
                    <li>{} views</li>
                    <li class="yt-lockup-deemphasized-text">{}</li>
                </ul>
            </div>
        </div>
    </div>
</li>"#,
        watch_url,
        thumb,
        h(&v.title),
        v.duration,
        watch_url,
        h(&v.title),
        h(&v.title),
        v.views,
        v.published_at
    )
}

fn render_channel_videos(videos: &[ChannelVideo], main_url: &str) -> String {
    if videos.is_empty() {
        return r#"<ul class="yt-uix-shelfslider-list">
                <div class="yt-alert yt-alert-default"><div class="yt-alert-content">No videos found for this channel.</div></div>
            </ul>"#.to_string();
    }
    let mut out = String::new();
    for chunk in videos.chunks(VIDEOS_PER_ROW) {
        out.push_str("<ul class=\"yt-uix-shelfslider-list\">\n");
        for v in chunk {
            out.push_str(&render_channel_video_item(v, main_url));
        }
        out.push_str("</ul>\n");
    }
    out
}

#[derive(serde::Deserialize)]
pub struct ChannelQuery {
    handle: Option<String>,
}

/// Normalize channel handle: remove leading @ or %40 (URL-encoded @).
fn normalize_channel_handle(handle: &str) -> String {
    let s = handle.trim();
    let s = s.strip_prefix('@').unwrap_or(s);
    let s = s.strip_prefix("%40").unwrap_or(s);
    s.to_string()
}

pub async fn page_channel(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    query: web::Query<ChannelQuery>,
) -> impl Responder {
    let handle = match &query.handle {
        Some(h) if !h.is_empty() => normalize_channel_handle(h),
        _ => {
            return HttpResponse::BadRequest()
                .content_type("text/html; charset=utf-8")
                .body("<h1>Missing channel</h1><p>Use ?handle=CHANNEL_HANDLE</p>");
        }
    };

    let config = &data.config;
    let base = base_url(&req, config);
    let main_url = base.clone();

    let channel_response: ChannelVideosResponse = match fetch_json(
        &base,
        &format!("/get_author_videos.php?author={}", urlencoding::encode(&handle)),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::log::info!("Frontend channel: failed to fetch channel: {}", e);
            return HttpResponse::InternalServerError()
                .content_type("text/html; charset=utf-8")
                .body(format!("<h1>Channel not found</h1><p>{}</p>", h(&e)));
        }
    };

    let channel_info = &channel_response.channel_info;
    let videos = &channel_response.videos;

    let channel_title = &channel_info.title;
    let channel_description = &channel_info.description;
    let channel_thumbnail = if channel_info.thumbnail.is_empty() {
        "/assets/images/photo.jpg"
    } else {
        &channel_info.thumbnail
    };
    let channel_banner = &channel_info.banner;
    let subscriber_count = &channel_info.subscriber_count;
    let channel_url = format!("{}/channel?handle={}", main_url, urlencoding::encode(&handle));

    let navbar = render_navbar(&main_url, "");
    let sidebar_html = render_sidebar(&main_url, None);
    let spotlight_html = render_spotlight_html(videos, &main_url);
    let videos_html = render_channel_videos(videos, &main_url);

    let t = load_template("channel");
    let html = t
        .replace("{{NAVBAR}}", &navbar)
        .replace("{{SIDEBAR}}", &sidebar_html)
        .replace("{{MAIN_URL}}", &main_url)
        .replace("{{CHANNEL_TITLE}}", &h(channel_title))
        .replace("{{CHANNEL_DESCRIPTION}}", &h(channel_description))
        .replace("{{CHANNEL_THUMBNAIL}}", channel_thumbnail)
        .replace("{{CHANNEL_BANNER}}", channel_banner)
        .replace("{{SUBSCRIBER_COUNT}}", subscriber_count)
        .replace("{{CHANNEL_URL}}", &channel_url)
        .replace("{{SPOTLIGHT_HTML}}", &spotlight_html)
        .replace("{{VIDEOS_HTML}}", &videos_html);

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

// ---- Login: sign-in page with navbar, sidebar, QR code auth (IE-compatible) ----
pub async fn page_login(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let main_url = base_url(&req, config);
    let navbar = render_navbar(&main_url, "");
    let sidebar_html = render_sidebar(&main_url, None);
    let t = load_template("login");
    let html = t
        .replace("{{NAVBAR}}", &navbar)
        .replace("{{SIDEBAR}}", &sidebar_html)
        .replace("{{MAIN_URL}}", &main_url);
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

// ---- Logout: clear session token, clear cookie, redirect to login ----
pub async fn page_logout(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    token_store: web::Data<TokenStore>,
) -> impl Responder {
    if let Some(cookie) = req.cookie("session_id") {
        token_store.remove_token(cookie.value());
    }
    let config = &data.config;
    let main_url = base_url(&req, config);
    let login_url = format!("{}/auth/login", main_url);
    HttpResponse::Found()
        .insert_header(("Location", login_url))
        .insert_header((
            "Set-Cookie",
            "session_id=; Path=/; Max-Age=0",
        ))
        .finish()
}

// ---- Embed: iframe player for watch page (yt2014 embed with same styles) ----
pub async fn page_embed(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let video_id = path.into_inner();
    if video_id.is_empty() {
        return HttpResponse::BadRequest()
            .content_type("text/html; charset=utf-8")
            .body("<h1>Missing video ID</h1>");
    }
    let config = &data.config;
    let base = base_url(&req, config);
    let video_src = format!(
        "{}/direct_url?video_id={}",
        base.trim_end_matches('/'),
        urlencoding::encode(&video_id)
    );
    let poster = format!("{}/thumbnail/{}", base.trim_end_matches('/'), urlencoding::encode(&video_id));
    let t = load_template("embed");
    let html = t
        .replace("{{VIDEO_SRC}}", &h(&video_src))
        .replace("{{POSTER}}", &h(&poster));
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}
