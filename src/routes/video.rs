use actix_web::http::header::{HeaderValue, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION};
use actix_web::{web, HttpRequest, HttpResponse, Responder};
use bytes::Bytes;
use futures_util::StreamExt;
use std::io::{Read, Seek, Write};
use std::process::Stdio;
use std::env;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use html_escape::decode_html_entities;
use image::{GenericImageView, Pixel};
use lazy_static::lazy_static;
use lru::LruCache;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::task;
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

fn extract_ytcfg(html: &str) -> serde_json::Value {
    if let Some(cap) = regex::Regex::new(r"ytcfg\.set\((\{.*?\})\);")
        .unwrap()
        .captures(html)
    {
        if let Ok(cfg) = serde_json::from_str(&cap[1]) {
            return cfg;
        }
    }
    serde_json::Value::Object(serde_json::Map::new())
}

fn extract_initial_player_response(html: &str) -> serde_json::Value {
    let patterns = [
        r"ytInitialPlayerResponse\s*=\s*(\{.+?\});",
        r"window\['ytInitialPlayerResponse'\]\s*=\s*(\{.+?\});",
    ];
    
    for pattern in &patterns {
        if let Some(cap) = regex::Regex::new(pattern)
            .unwrap()
            .captures(html)
        {
            if let Ok(pr) = serde_json::from_str(&cap[1]) {
                return pr;
            }
        }
    }
    serde_json::Value::Object(serde_json::Map::new())
}

fn get_comments_token(data: &serde_json::Value) -> Option<String> {
    if let Some(contents) = data
        .get("contents")
        .and_then(|c| c.get("twoColumnWatchNextResults"))
        .and_then(|c| c.get("results"))
        .and_then(|r| r.get("results"))
        .and_then(|r| r.get("contents"))
        .and_then(|c| c.as_array())
    {
        for item in contents {
            if let Some(item_section) = item.get("itemSectionRenderer") {
                if item_section
                    .get("sectionIdentifier")
                    .and_then(|s| s.as_str())
                    .map(|s| s == "comment-item-section")
                    .unwrap_or(false)
                {
                    if let Some(section_contents) = item_section
                        .get("contents")
                        .and_then(|c| c.as_array())
                    {
                        for content_item in section_contents {
                            if content_item.get("continuationItemRenderer").is_some() {
                                return content_item
                                    .get("continuationItemRenderer")
                                    .and_then(|c| c.get("continuationEndpoint"))
                                    .and_then(|e| e.get("continuationCommand"))
                                    .and_then(|c| c.get("token"))
                                    .and_then(|t| t.as_str())
                                    .map(|s| s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn simplify_text(node: &serde_json::Value) -> String {
    if node.is_null() {
        return String::new();
    }
    if let Some(s) = node.as_str() {
        return s.to_string();
    }
    if let Some(simple_text) = node.get("simpleText").and_then(|t| t.as_str()) {
        return simple_text.to_string();
    }
    if let Some(runs) = node.get("runs").and_then(|r| r.as_array()) {
        let mut result = String::new();
        for run in runs {
            if let Some(text) = run.get("text").and_then(|t| t.as_str()) {
                result.push_str(text);
            }
        }
        return result;
    }
    String::new()
}

fn recursive_find(obj: &serde_json::Value, key: &str) -> Vec<serde_json::Value> {
    let mut found = Vec::new();
    if let Some(obj_map) = obj.as_object() {
        if obj_map.contains_key(key) {
            found.push(obj_map[key].clone());
        }
        for value in obj_map.values() {
            found.extend(recursive_find(value, key));
        }
    } else if let Some(arr) = obj.as_array() {
        for item in arr {
            found.extend(recursive_find(item, key));
        }
    }
    found
}

fn all_strings(obj: &serde_json::Value) -> Vec<String> {
    let mut strings = Vec::new();
    if let Some(obj_map) = obj.as_object() {
        for value in obj_map.values() {
            strings.extend(all_strings(value));
        }
    } else if let Some(arr) = obj.as_array() {
        for item in arr {
            strings.extend(all_strings(item));
        }
    } else if let Some(s) = obj.as_str() {
        strings.push(s.to_string());
    }
    strings
}

fn search_number_near(data: &serde_json::Value, words: &[&str]) -> String {
    for s in all_strings(data) {
        let sl = s.to_lowercase();
        if words.iter().any(|w| sl.contains(w)) {
            if let Some(captures) = regex::Regex::new(r"[\d][\d,. ]*").unwrap().captures(&s) {
                return captures[0].replace(" ", "").replace(",", "");
            }
        }
    }
    String::new()
}

fn find_likes(next_data: &serde_json::Value) -> String {
    if let Some(contents) = next_data
        .get("contents")
        .and_then(|c| c.get("twoColumnWatchNextResults"))
        .and_then(|c| c.get("results"))
        .and_then(|r| r.get("results"))
        .and_then(|r| r.get("contents"))
        .and_then(|c| c.as_array())
    {
        if !contents.is_empty() {
            if let Some(primary_info) = contents[0].get("videoPrimaryInfoRenderer") {
                if let Some(video_actions) = primary_info.get("videoActions") {
                    if let Some(menu_renderer) = video_actions.get("menuRenderer") {
                        if let Some(top_level_buttons) = menu_renderer.get("topLevelButtons").and_then(|btns| btns.as_array()) {
                            if !top_level_buttons.is_empty() {
                                if let Some(button) = top_level_buttons[0].get("segmentedLikeDislikeButtonViewModel") {
                                    if let Some(like_button_vm) = button.get("likeButtonViewModel") {
                                        if let Some(like_button_vm2) = like_button_vm.get("likeButtonViewModel") {
                                            if let Some(toggle_button_vm) = like_button_vm2.get("toggleButtonViewModel") {
                                                if let Some(toggle_button_vm2) = toggle_button_vm.get("toggleButtonViewModel") {
                                                    if let Some(toggled_btn) = toggle_button_vm2.get("toggledButtonViewModel") {
                                                        if let Some(button_vm) = toggled_btn.get("buttonViewModel",) {
                                                            if let Some(title) = button_vm.get("title").and_then(|t| t.as_str()) {
                                                                if !title.is_empty() && title.chars().any(|c| c.is_ascii_digit()) {
                                                                    println!("DEBUG: взято из toggled.title = {}", title);
                                                                    return parse_human_number(title);
                                                                }
                                                            }
                                                            
                                                            if let Some(acc_text) = button_vm.get("accessibilityText").and_then(|t| t.as_str()) {
                                                                if !acc_text.is_empty() {
                                                                    if let Some(caps) = regex::Regex::new(r"along with ([\d, ]*) other").unwrap().captures(acc_text) {
                                                                        let num = caps[1].replace(",", "").replace(" ", "");
                                                                        println!("DEBUG: взято из accessibility = {}", num);
                                                                        return num;
                                                                    }
                                                                    if let Some(caps) = regex::Regex::new(r"(\d[\d, ]*)").unwrap().captures(acc_text) {
                                                                        return parse_human_number(&caps[1]);
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    
                                                    if let Some(default_btn) = toggle_button_vm2.get("defaultButtonViewModel") {
                                                        if let Some(button_vm) = default_btn.get("buttonViewModel") {
                                                            if let Some(title) = button_vm.get("title").and_then(|t| t.as_str()) {
                                                                if !title.is_empty() && title.chars().any(|c| c.is_ascii_digit()) {
                                                                    println!("DEBUG: взято из default.title = {}", title);
                                                                    return parse_human_number(title);
                                                                }
                                                            }
                                                            
                                                            if let Some(acc_text) = button_vm.get("accessibilityText").and_then(|t| t.as_str()) {
                                                                if !acc_text.is_empty() {
                                                                    if let Some(caps) = regex::Regex::new(r"along with ([\d, ]*) other").unwrap().captures(acc_text) {
                                                                        let num = caps[1].replace(",", "").replace(" ", "");
                                                                        println!("DEBUG: взято из accessibility = {}", num);
                                                                        return num;
                                                                    }
                                                                    if let Some(caps) = regex::Regex::new(r"(\d[\d, ]*)").unwrap().captures(acc_text) {
                                                                        return parse_human_number(&caps[1]);
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
                        }
                    }
                }
            }
        }
    }
    
    if let Some(micro) = next_data
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"))
    {
        if let Some(like_count) = micro.get("likeCount").and_then(|lc| lc.as_str()) {
            return like_count.to_string();
        }
    }
    
    search_number_near(next_data, &["like", "likes", "лайк", "лайков", "лайка"])
}

fn parse_human_number(s: &str) -> String {
    if s.is_empty() {
        return "0".to_string();
    }
    
    let trimmed = s.trim();
    let mut cleaned = String::with_capacity(trimmed.len());
    
    for c in trimmed.chars() {
        if c != ',' && c != ' ' {
            cleaned.push(c.to_ascii_uppercase());
        }
    }
    
    if cleaned.len() > 1 {
        let last_char = cleaned.chars().last().unwrap();
        if last_char.is_alphabetic() {
            let num_part = &cleaned[..cleaned.len()-1];
            match last_char {
                'K' => {
                    if let Ok(num) = num_part.parse::<f64>() {
                        return ((num * 1000.0).round() as i64).to_string();
                    }
                },
                'M' => {
                    if let Ok(num) = num_part.parse::<f64>() {
                        return ((num * 1000000.0).round() as i64).to_string();
                    }
                },
                'B' => {
                    if let Ok(num) = num_part.parse::<f64>() {
                        return ((num * 1000000000.0).round() as i64).to_string();
                    }
                },
                _ => {} // Not a recognized multiplier
            }
        }
    }
    
    let mut result = String::new();
    for c in cleaned.chars() {
        if c.is_ascii_digit() {
            result.push(c);
        }
    }
    result
}

fn find_subscriber_count(nd: &serde_json::Value) -> String {
    
    if let Some(contents) = nd.get("contents") {
        if let Some(two_col) = contents.get("twoColumnWatchNextResults") {
            if let Some(results) = two_col.get("results") {
                if let Some(results2) = results.get("results") {
                    if let Some(contents_array) = results2.get("contents").and_then(|c| c.as_array()) {
                        if contents_array.len() > 1 {
                            let item1 = &contents_array[1];
                            
                            if let Some(video_secondary) = item1.get("videoSecondaryInfoRenderer") {
                                if let Some(owner) = video_secondary.get("owner") {
                                    if let Some(video_owner) = owner.get("videoOwnerRenderer") {
                                        if let Some(sub_text) = video_owner.get("subscriberCountText") {
                                            let text = sub_text
                                                .get("simpleText")
                                                .and_then(|t| t.as_str())
                                                .or_else(|| {
                                                    sub_text.get("runs").and_then(|r| r.as_array()).and_then(|arr| arr.first()).and_then(|r| r.get("text").and_then(|t| t.as_str()))
                                                });
                                            if let Some(simple_text) = text {
                                                let cleaned = simple_text.replace(" подписчиков", "").replace(" подписчик", "").replace(" subscribers", "").replace(" subscriber", "");
                                                
                                                let re = regex::Regex::new(r"([\d,]+\.?\d*)\s*(млн|тыс|[KM]?)").unwrap();
                                                if let Some(captures) = re.captures(&cleaned) {
                                                    let number_part = &captures[1].replace(",", "").replace(".", ""); // Remove commas and dots
                                                    let multiplier = &captures[2];
                                                    
                                                    if let Ok(number) = number_part.parse::<f64>() {
                                                        let result = match multiplier {
                                                            "млн" => (number * 1000000.0) as u64, // Russian million
                                                            "тыс" => (number * 1000.0) as u64,    // Russian thousand
                                                            "K" => (number * 1000.0) as u64,      // English thousand
                                                            "M" => (number * 1000000.0) as u64,   // English million
                                                            _ => number as u64,                   // No multiplier
                                                        };
                                                        return result.to_string();
                                                    }
                                                }
                                                let digits: String = cleaned.chars().filter(|c| c.is_ascii_digit()).collect();
                                                if !digits.is_empty() {
                                                    return digits;
                                                }
                                                return cleaned;
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
    
    "0".to_string()
}

fn find_comments_count(pr: &serde_json::Value, nd: &serde_json::Value) -> String {
    if let Some(panels) = nd.get("engagementPanels").and_then(|p| p.as_array()) {
        for panel in panels {
            if let Some(panel_renderer) = panel.get("engagementPanelSectionListRenderer") {
                if let Some(identifier) = panel_renderer.get("panelIdentifier").and_then(|id| id.as_str()) {
                    if identifier == "engagement-panel-comments-section" {
                        if let Some(header) = panel_renderer.get("header") {
                            if let Some(title_header_renderer) = header.get("engagementPanelTitleHeaderRenderer") {
                                if let Some(contextual_info) = title_header_renderer.get("contextualInfo") {
                                    if let Some(runs) = contextual_info.get("runs").and_then(|r| r.as_array()) {
                                        if !runs.is_empty() {
                                            if let Some(first_run) = runs[0].get("text").and_then(|t| t.as_str()) {
                                                let result = first_run.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
                                                if !result.is_empty() {
                                                    return result;
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
    }
    
    for d in [pr, nd] {
        if d.is_null() {
            continue;
        }
        
        let comment_texts = recursive_find(d, "commentCountText");
        let count_texts = recursive_find(d, "countText");
        
        let all_texts: Vec<&serde_json::Value> = comment_texts.iter().chain(count_texts.iter()).collect();
        
        for ct in all_texts {
            let text = ct
                .get("simpleText")
                .and_then(|st| st.as_str())
                .or_else(|| {
                    ct.get("runs")
                        .and_then(|r| r.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|first_run| first_run.get("text"))
                        .and_then(|t| t.as_str())
                });
            
            if let Some(text_content) = text {
                let n = text_content.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
                if !n.is_empty() {
                    return n;
                }
            }
        }
    }
    search_number_near(nd, &["comment", "comments", "коммент", "коммента"])
}

fn translate_russian_time(time_str: &str) -> String {
    let time_lower = time_str.to_lowercase();
    
    let translations = [
        ("несколько секунд назад", "a few seconds ago"),
        ("секунду назад", "a second ago"),
        (" секунд назад", " seconds ago"),
        (" секунды назад", " seconds ago"),
        (" минуту назад", " a minute ago"),
        (" минут назад", " minutes ago"),
        (" часа назад", " hours ago"),
        (" часов назад", " hours ago"),
        (" день назад", " a day ago"),
        (" дней назад", " days ago"),
        (" недели назад", " weeks ago"),
        (" недель назад", " weeks ago"),
        (" месяц назад", " a month ago"),
        (" месяцев назад", " months ago"),
        (" года назад", " years ago"),
        (" лет назад", " years ago"),
        ("только что", "just now"),
        ("сегодня", "today"),
        ("вчера", "yesterday"),
        ("позавчера", "day before yesterday"),
    ];
    
    let mut result = time_str.to_string();
    for (russian, english) in &translations {
        if time_lower.contains(russian) {
            result = result.replace(russian, english);
            let capitalized_russian = format!("{}{}", 
                russian.chars().next().unwrap().to_uppercase().collect::<String>(),
                &russian[1..]
            );
            result = result.replace(&capitalized_russian, english);
        }
    }
    
    result
}

fn extract_comments(data: &serde_json::Value, base_url: &str) -> Vec<Comment> {
    let mut comments = Vec::new();
    
    fn walk(obj: &serde_json::Value, comments: &mut Vec<Comment>, base_url: &str) {
        if let Some(obj_map) = obj.as_object() {
            if obj_map.contains_key("commentEntityPayload") {
                let p = &obj_map["commentEntityPayload"];
                let props = p.get("properties").unwrap_or(&serde_json::Value::Null);
                
                let author = p
                    .get("author")
                    .and_then(|a| a.get("displayName"))
                    .and_then(|d| d.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Unknown")
                    .to_string();
                
                let text = if let Some(content_obj) = p.get("properties").and_then(|props| props.get("content")) {
                    if let Some(content_str) = content_obj.get("content").and_then(|c| c.as_str()) {
                        content_str.to_string()
                    } else if let Some(runs) = content_obj.get("runs").and_then(|r| r.as_array()) {
                        let mut text = String::new();
                        for run in runs {
                            if let Some(run_text) = run.get("text").and_then(|t| t.as_str()) {
                                text.push_str(run_text);
                            }
                        }
                        text
                    } else {
                        String::new()
                    }
                } else {
                    let content = props.get("content").unwrap_or(&serde_json::Value::Null);
                    if let Some(runs) = content.get("runs").and_then(|r| r.as_array()) {
                        let mut text = String::new();
                        for run in runs {
                            if let Some(run_text) = run.get("text").and_then(|t| t.as_str()) {
                                text.push_str(run_text);
                            }
                        }
                        text
                    } else {
                        String::new()
                    }
                };
                
                if !text.trim().is_empty() {
                    let published_at_raw = props
                        .get("publishedTime")
                        .and_then(|p| p.as_str())
                        .unwrap_or("unknown");
                    
                    let published_at = translate_russian_time(published_at_raw);
                    
                    let author_thumbnail_raw = p
                        .get("avatar")
                        .and_then(|a| a.get("image"))
                        .and_then(|i| i.get("sources"))
                        .and_then(|s| s.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|src| src.get("url"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    
                    let author_thumbnail = if !author_thumbnail_raw.is_empty() {
                        format!("{}/channel_icon/{}", base_url, urlencoding::encode(author_thumbnail_raw))
                    } else {
                        String::new()
                    };
                    
                    comments.push(Comment {
                        author,
                        text: text.trim().to_string(),  // Only trim if necessary
                        published_at,
                        author_thumbnail,
                        author_channel_url: None,
                    });
                }
            }
            for value in obj_map.values() {
                walk(value, comments, base_url);
            }
        } else if let Some(arr) = obj.as_array() {
            for item in arr {
                walk(item, comments, base_url);
            }
        }
    }
    
    walk(data, &mut comments, base_url);
    comments
}

lazy_static! {
    static ref THUMBNAIL_CACHE: Arc<Mutex<LruCache<String, (Vec<u8>, String, u64)>>> = Arc::new(
        Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1000).unwrap()))
    );
    static ref DIRECT_URL_CLEANUP_STARTED: AtomicBool = AtomicBool::new(false);
}

const CACHE_DURATION: u64 = 3600;

fn yt_dlp_binary() -> String {
    if cfg!(target_os = "windows") {
        if Path::new("assets/yt-dlp.exe").exists() {
            return "assets/yt-dlp.exe".to_string();
        }
    } else if Path::new("assets/yt-dlp").exists() {
        return "assets/yt-dlp".to_string();
    }
    "yt-dlp".to_string()
}

fn sanitize_text(input: &str) -> String {
    let decoded = urlencoding::decode(input)
        .unwrap_or_else(|_| input.into())
        .to_string();
    let decoded = decode_html_entities(&decoded).to_string();
    
    let mut result = String::new();
    let mut prev_was_space = false;
    
    for c in decoded.chars() {
        if c.is_whitespace() {
            if !prev_was_space && !result.is_empty() {
                result.push(' ');
                prev_was_space = true;
            }
        } else if !c.is_control() {
            result.push(c);
            prev_was_space = false;
        }
    }
    
    result
}

async fn dominant_color_from_url(url: &str) -> Option<String> {
    let client = Client::new();
    let bytes = client.get(url).send().await.ok()?.bytes().await.ok()?;
    let vec = bytes.to_vec();
    task::spawn_blocking(move || {
        let img = image::load_from_memory(&vec).ok()?;
        let mut r: u64 = 0;
        let mut g: u64 = 0;
        let mut b: u64 = 0;
        let mut count: u64 = 0;
        for pixel in img.pixels() {
            let rgb = pixel.2.to_rgb();
            r += rgb[0] as u64;
            g += rgb[1] as u64;
            b += rgb[2] as u64;
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let r = (r / count) as u8;
        let g = (g / count) as u8;
        let b = (b / count) as u8;
        Some(format!("#{:02x}{:02x}{:02x}", r, g, b))
    })
    .await
    .ok()
    .flatten()
}

fn collect_cookie_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir("cookies") {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(ext) = p.extension() {
                    if ext == "txt" {
                        paths.push(p);
                    }
                }
            }
        }
    }
    let legacy = ["assets/cookies.txt", "cookies.txt"];
    for p in legacy {
        let pb = PathBuf::from(p);
        if pb.exists() {
            paths.push(pb);
        }
    }
    paths
}

fn parse_quality_height(quality: &str) -> Option<u32> {
    let s = quality.trim().to_lowercase();
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Ok(h) = digits.parse::<u32>() {
            return Some(h);
        }
    }
    let aliases: std::collections::HashMap<&str, u32> = [
        ("tiny", 144),
        ("small", 240),
        ("medium", 360),
        ("large", 480),
        ("hd", 720),
        ("hd720", 720),
        ("720p", 720),
        ("hd1080", 1080),
        ("1080p", 1080),
        ("144p", 144),
        ("240p", 240),
        ("360p", 360),
        ("480p", 480),
        ("2160p", 2160),
        ("1440p", 1440),
    ]
    .into_iter()
    .collect();
    aliases.get(s.as_str()).copied()
}

async fn resolve_video_audio_urls(
    video_id: &str,
    height: u32,
    config: &crate::config::Config,
) -> Result<(String, String), String> {
    let video_id = video_id.to_string();
    let use_cookies = config.video.use_cookies;
    let yt_dlp = yt_dlp_binary();
    let mut cookie_paths = Vec::new();
    if use_cookies {
        cookie_paths = collect_cookie_paths();
    }

    let mut attempts: Vec<Option<PathBuf>> = Vec::new();
    for p in &cookie_paths {
        attempts.push(Some(p.clone()));
    }
    attempts.push(None);

    for cookie in attempts {
        let cookie_for_dump = cookie.clone();
        let url = format!("https://www.youtube.com/watch?v={}", video_id);
        let result = task::spawn_blocking({
            let yt_dlp = yt_dlp.clone();
            let url = url.clone();
            move || {
                let mut cmd = Command::new(&yt_dlp);
                cmd.arg("--dump-json").arg(&url);
                if let Some(ref path) = cookie_for_dump {
                    cmd.arg("--cookies").arg(path);
                }
                let output = cmd.output().ok().filter(|o| o.status.success())?;
                let info: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
                let formats = info.get("formats")?.as_array()?;
                let mut video_candidates: Vec<(f64, String)> = Vec::new();
                let mut video_fallback_candidates: Vec<(u32, f64, String)> = Vec::new();
                let mut audio_format_id: Option<String> = None;
                let mut best_audio_tbr = 0f64;
                let mut best_audio_is_en = false;
                for f in formats {
                    let h = f.get("height").and_then(|v| v.as_u64()).map(|u| u as u32);
                    let vcodec = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
                    let acodec = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
                    let protocol = f.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
                    if !protocol.starts_with("https") {
                        continue;
                    }
                    if vcodec != "none" && acodec == "none" {
                        if let Some(hi) = h {
                            if let Some(id) = f.get("format_id").and_then(|v| v.as_str()) {
                                let tbr = f.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                if hi == height {
                                    video_candidates.push((tbr, id.to_string()));
                                } else if hi <= height {
                                    video_fallback_candidates.push((hi, tbr, id.to_string()));
                                }
                            }
                        }
                    }
                    if vcodec == "none" && acodec != "none" {
                        let format_note = f.get("format").and_then(|v| v.as_str()).unwrap_or("");
                        let is_en = format_note.contains("[en]");
                        let tbr = f.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let id = f.get("format_id").and_then(|v| v.as_str()).map(String::from);
                        if let Some(id) = id {
                            let replace = audio_format_id.is_none()
                                || is_en && !best_audio_is_en
                                || (is_en == best_audio_is_en && tbr > best_audio_tbr);
                            if replace {
                                audio_format_id = Some(id);
                                best_audio_tbr = tbr;
                                best_audio_is_en = is_en;
                            }
                        }
                    }
                }
                let video_format_id = video_candidates
                    .into_iter()
                    .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(_, id)| id)
                    .or_else(|| {
                        video_fallback_candidates
                            .into_iter()
                            .max_by(|a, b| {
                                a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                            })
                            .map(|(_, _, id)| id)
                    });
                let video_fid = video_format_id?;
                let audio_id = audio_format_id?;
                Some((video_fid, audio_id))
            }
        })
        .await
        .map_err(|e| e.to_string())?;

        let (video_fid, audio_fid) = match result {
            Some(x) => x,
            None => continue,
        };

        let cookie_for_get_url = cookie.clone();
        let (video_url, audio_url) = task::spawn_blocking({
            let yt_dlp = yt_dlp.clone();
            let url = url.clone();
            move || {
                let mut cmd_v = Command::new(&yt_dlp);
                cmd_v.arg("-f").arg(&video_fid).arg("--get-url").arg(&url);
                if let Some(ref path) = cookie_for_get_url {
                    cmd_v.arg("--cookies").arg(path);
                }
                let out_v = cmd_v.output().ok().filter(|o| o.status.success())?;
                let url_v = String::from_utf8_lossy(&out_v.stdout);
                let url_v = url_v.lines().find(|l| !l.trim().is_empty())?.trim().to_string();
                let mut cmd_a = Command::new(&yt_dlp);
                cmd_a.arg("-f").arg(&audio_fid).arg("--get-url").arg(&url);
                if let Some(ref path) = cookie_for_get_url {
                    cmd_a.arg("--cookies").arg(path);
                }
                let out_a = cmd_a.output().ok().filter(|o| o.status.success())?;
                let url_a = String::from_utf8_lossy(&out_a.stdout);
                let url_a = url_a.lines().find(|l| !l.trim().is_empty())?.trim().to_string();
                Some((url_v, url_a))
            }
        })
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "yt-dlp failed to get video or audio URL".to_string())?;

        return Ok((video_url, audio_url));
    }

    Err("Could not get separate video and audio URLs for any cookie attempt".to_string())
}

fn stream_ffmpeg_merged_response(video_url: &str, audio_url: &str) -> HttpResponse {
    const CHUNK: usize = 65536;
    let video_url = video_url.to_string();
    let audio_url = audio_url.to_string();
    let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(8);
    let user_agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0 Safari/537.36";
    let headers_arg = "Referer: https://www.youtube.com\r\nOrigin: https://www.youtube.com";
    std::thread::spawn(move || {
        let mut child = match Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-reconnect",
                "1",
                "-reconnect_streamed",
                "1",
                "-reconnect_at_eof",
                "1",
                "-reconnect_delay_max",
                "10",
                "-user_agent",
                user_agent,
                "-headers",
                headers_arg,
                "-i",
                &video_url,
                "-user_agent",
                user_agent,
                "-headers",
                headers_arg,
                "-i",
                &audio_url,
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "copy",
                "-c:a",
                "aac",
                "-b:a",
                "160k",
                "-movflags",
                "frag_keyframe+empty_moov",
                "-f",
                "mp4",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.try_send(Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("ffmpeg spawn failed: {}", e),
                )));
                return;
            }
        };
        if let Some(mut stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let mut line = String::new();
                let mut buf = [0u8; 512];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    for &b in &buf[..n] {
                        if b == b'\n' || b == b'\r' {
                            if !line.is_empty() {
                                line.clear();
                            }
                        } else {
                            line.push(b as char);
                        }
                    }
                }
                if !line.is_empty() {}
            });
        }
        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => return,
        };
        let mut buf = [0u8; CHUNK];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(Ok(Bytes::from(buf[..n].to_vec()))).is_err() {
                        let _ = child.kill();
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.try_send(Err(e));
                    let _ = child.kill();
                    break;
                }
            }
        }
    });
    let stream = ReceiverStream::new(rx).map(|r| r.map(web::Bytes::from).map_err(actix_web::error::ErrorInternalServerError));
    HttpResponse::Ok()
        .insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")))
        .insert_header(("Accept-Ranges", "bytes"))
        .streaming(stream)
}

fn stream_converted_video(
    source_url: &str,
    user_agent: &str,
    _video_id: &str,
    codec: &str,
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> HttpResponse {
    let source_url = source_url.to_string();
    let ua = user_agent.to_string();
    let codec_str = codec.to_string();
    let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(8);

    std::thread::spawn(move || {
        let _permit = _permit; // moved into thread; dropped when thread exits
        let temp_dir = env::temp_dir();
        let temp_file_name = format!(
            "yt_api_video_{}_{}.{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            std::process::id(),
            if codec_str == "mpeg4" { "mp4" } else { "3gp" }
        );
        let temp_file_path = temp_dir.join(temp_file_name);

        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-hide_banner", "-loglevel", "error", "-nostdin",
            "-reconnect", "1",
            "-reconnect_streamed", "1",
            "-reconnect_at_eof", "1",
            "-reconnect_delay_max", "10",
            "-user_agent", &ua,
            "-headers", "Referer: https://www.youtube.com\r\nOrigin: https://www.youtube.com",
            "-i", &source_url,
        ]);

        if codec_str == "mpeg4" {
            cmd.args([
                "-c:v", "mpeg4", "-vtag", "mp4v", "-b:v", "501k",
                "-brand", "isom", "-pix_fmt", "yuv420p",
                "-c:a", "copy", "-f", "mp4",
            ]);
        } else {
            cmd.args([
                "-c:v", "h263", "-vf", "scale=352:288",
                "-c:a", "libopencore_amrnb", "-ar", "8000", "-ac", "1",
                "-f", "3gp",
            ]);
        }

        let temp_path_str = temp_file_path.to_string_lossy().to_string();
        cmd.arg(&temp_path_str);

        cmd.stdin(Stdio::null())
            .stderr(Stdio::piped());

        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                let _ = tx.blocking_send(Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("FFmpeg failed to start: {}", e)
                )));
                let _ = fs::remove_file(&temp_file_path);
                return;
            }
        };

        if !output.status.success() {
            let err_msg = String::from_utf8_lossy(&output.stderr).to_string();
            log::error!("FFmpeg conversion failed: {}", err_msg);
            let _ = tx.blocking_send(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("FFmpeg conversion failed: {}", err_msg)
            )));
            let _ = fs::remove_file(&temp_file_path);
            return;
        }

        match fs::File::open(&temp_file_path) {
            Ok(mut file) => {
                let mut buffer = [0u8; 65536];
                loop {
                    match file.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.blocking_send(Ok(Bytes::copy_from_slice(&buffer[..n]))).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx.blocking_send(Err(e));
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                let _ = tx.blocking_send(Err(e));
            }
        }

        let _ = fs::remove_file(&temp_file_path);
    });

    let mime_type = if codec == "mpeg4" { "video/mp4" } else { "video/3gpp" };
    let stream = ReceiverStream::new(rx).map(|r| r.map(web::Bytes::from).map_err(actix_web::error::ErrorInternalServerError));
    HttpResponse::Ok()
        .insert_header((CONTENT_TYPE, HeaderValue::from_str(mime_type).unwrap()))
        .insert_header(("Cache-Control", "public, max-age=3600"))
        .streaming(stream)
}

/// Removes old temp files created by direct_url: `yt_api_video_*` in temp_dir (older than 1h),
/// and files in `yt_api_hls_cache` older than 24h.
fn clean_direct_url_temp_files() {
    let temp_dir = env::temp_dir();
    let now = SystemTime::now();
    let max_age_video = Duration::from_secs(3600);   // 1 hour for codec conversion temp files
    let max_age_hls = Duration::from_secs(86400);   // 24 hours for HLS cache

    if let Ok(entries) = fs::read_dir(&temp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("yt_api_video_") && (name.ends_with(".mp4") || name.ends_with(".3gp")) {
                        if let Ok(meta) = fs::metadata(&path) {
                            if let Ok(mtime) = meta.modified() {
                                if now.duration_since(mtime).unwrap_or(Duration::MAX) > max_age_video {
                                    let _ = fs::remove_file(&path);
                                    log::debug!("direct_url cleanup: removed old temp {}", path.display());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let hls_cache = temp_dir.join("yt_api_hls_cache");
    if hls_cache.is_dir() {
        if let Ok(entries) = fs::read_dir(&hls_cache) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = fs::metadata(&path) {
                    if let Ok(mtime) = meta.modified() {
                        if now.duration_since(mtime).unwrap_or(Duration::MAX) > max_age_hls {
                            let _ = fs::remove_file(&path);
                            log::debug!("direct_url cleanup: removed old hls cache {}", path.display());
                        }
                    }
                }
            }
        }
    }
}

async fn direct_url_cleanup_loop() {
    let interval = Duration::from_secs(900); // 15 minutes
    loop {
        tokio::time::sleep(interval).await;
        let _ = task::spawn_blocking(clean_direct_url_temp_files).await;
    }
}

fn spawn_direct_url_cleanup_if_needed() {
    if DIRECT_URL_CLEANUP_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        actix_web::rt::spawn(direct_url_cleanup_loop());
    }
}

async fn resolve_direct_stream_url(
    video_id: &str,
    quality: Option<&str>,
    audio_only: bool,
    config: &crate::config::Config,
) -> Result<String, String> {
    let video_id = video_id.to_string();
    let quality = quality
        .map(|q| q.to_string())
        .unwrap_or_else(|| config.video.default_quality.clone());
    let use_cookies = config.video.use_cookies;
    let yt_dlp = yt_dlp_binary();
    let mut cookie_paths = Vec::new();
    if use_cookies {
        cookie_paths = collect_cookie_paths();
        let names: Vec<String> = cookie_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        if names.is_empty() {
            crate::log::info!("Cookies enabled; found 0 files");
        } else {
            crate::log::info!(
                "Cookies enabled; found {} files: {}",
                names.len(),
                names.join(", ")
            );
        }
    }

    task::spawn_blocking(move || {
        let url = format!("https://www.youtube.com/watch?v={}", video_id);
        let format_selector = if audio_only {
            "bestaudio/best".to_string()
        } else {
            format!("best[height<={}][ext=mp4]/best[ext=mp4]/best", quality)
        };

        let mut attempts: Vec<Option<PathBuf>> = Vec::new();
        for p in cookie_paths {
            attempts.push(Some(p));
        }
        attempts.push(None);

        let mut last_err = None;
        for cookie in attempts {
            let mut cmd = Command::new(&yt_dlp);
            cmd.arg("-f")
                .arg(&format_selector)
                .arg("--get-url")
                .arg(&url);

            if let Some(ref path) = cookie {
                cmd.arg("--cookies").arg(path);
            }

            match cmd.output() {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if let Some(line) = stdout.lines().find(|l| !l.trim().is_empty()) {
                        return Ok(line.to_string());
                    }
                    last_err = Some("yt-dlp returned empty output".to_string());
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let msg = if let Some(ref path) = cookie {
                        format!(
                            "yt-dlp failed with cookies {}: status {} stderr {}",
                            path.display(),
                            output.status,
                            stderr
                        )
                    } else {
                        format!(
                            "yt-dlp failed without cookies: status {} stderr {}",
                            output.status, stderr
                        )
                    };
                    crate::log::info!("{}", msg);
                    last_err = Some(msg);
                }
                Err(e) => {
                    let msg = if let Some(ref path) = cookie {
                        format!("yt-dlp exec error with cookies {}: {}", path.display(), e)
                    } else {
                        format!("yt-dlp exec error: {}", e)
                    };
                    crate::log::info!("{}", msg);
                    last_err = Some(msg);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| "yt-dlp failed for all attempts".to_string()))
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn proxy_stream_response(
    target_url: &str,
    req: &HttpRequest,
    default_content_type: &str,
) -> HttpResponse {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()
        .unwrap();

    let mut request_builder = client.get(target_url);
    if let Some(range_header) = req.headers().get("Range") {
        request_builder = request_builder.header("Range", range_header.clone());
    }

    match request_builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            let content_type = headers
                .get(CONTENT_TYPE)
                .and_then(|ct| ct.to_str().ok())
                .unwrap_or(default_content_type)
                .to_string();

            let stream = resp
                .bytes_stream()
                .map(|item| item.map_err(|e| actix_web::error::ErrorBadGateway(e)));

            let mut builder = HttpResponse::build(status);
            for (key, value) in headers.iter() {
                if key == "connection" || key == "transfer-encoding" {
                    continue;
                }
                builder.insert_header((key.clone(), value.clone()));
            }
            builder.insert_header((
                CONTENT_TYPE,
                HeaderValue::from_str(&content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            ));
            builder.streaming(stream)
        }
        Err(e) => {
            crate::log::info!("Proxy request failed: {}", e);
            HttpResponse::BadGateway().json(serde_json::json!({
                "error": "Failed to proxy request"
            }))
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct VideoInfoResponse {
    pub title: String,
    pub author: String,
    #[serde(rename = "subscriberCount")]
    pub subscriber_count: String,
    pub channel_custom_url: Option<String>,
    pub description: String,
    pub video_id: String,
    pub embed_url: String,
    pub duration: String,
    pub published_at: String,
    pub likes: Option<String>,
    pub views: Option<String>,
    pub comment_count: Option<String>,
    pub comments: Vec<Comment>,
    pub channel_thumbnail: String,
    pub thumbnail: String,
    pub video_url: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Comment {
    pub author: String,
    pub text: String,
    pub published_at: String,
    pub author_thumbnail: String,
    pub author_channel_url: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct RelatedVideo {
    pub title: String,
    pub author: String,
    pub video_id: String,
    pub views: String,
    pub published_at: String,
    pub thumbnail: String,
    pub channel_thumbnail: String,
    pub url: String,
    pub source: String,
    pub color: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DirectUrlResponse {
    pub video_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct HlsManifestUrlResponse {
    pub hls_manifest_url: String,
    pub video_id: String,
    pub message: Option<String>,
}

#[utoipa::path(
    get,
    path = "/thumbnail/{video_id}",
    params(
        ("video_id" = String, Path, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Thumbnail quality (default, medium, high, standard, maxres)")
    ),
    responses(
        (status = 200, description = "Thumbnail image", content_type = "image/jpeg"),
        (status = 404, description = "Thumbnail not found")
    )
)]
pub async fn thumbnail_proxy(path: web::Path<String>, req: HttpRequest) -> impl Responder {
    let video_id = path.into_inner();

    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let quality = query_params
        .get("quality")
        .map(|s| s.as_str())
        .unwrap_or("medium");

    let quality_map = [
        ("default", "default.jpg"),
        ("medium", "mqdefault.jpg"),
        ("high", "hqdefault.jpg"),
        ("standard", "sddefault.jpg"),
        ("maxres", "maxresdefault.jpg"),
    ];

    let thumbnail_type = quality_map
        .iter()
        .find(|(q, _)| *q == quality)
        .map(|(_, t)| *t)
        .unwrap_or("mqdefault.jpg");

    let cache_key = format!("{}_{}", video_id, thumbnail_type);

    {
        let mut cache = THUMBNAIL_CACHE.lock().await;
        if let Some((data, content_type, timestamp)) = cache.get(&cache_key) {
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if current_time - timestamp < CACHE_DURATION {
                return HttpResponse::Ok()
                    .content_type(content_type.as_str())
                    .body(data.clone());
            }
        }
    }

    let url = format!("https://i.ytimg.com/vi/{}/{}", video_id, thumbnail_type);

    let client = Client::new();

    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let headers = resp.headers().clone();
            if status == 404 && thumbnail_type != "mqdefault.jpg" {
                let fallback_url = format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", video_id);
                match client.get(&fallback_url).send().await {
                    Ok(fallback_resp) => {
                        let fallback_headers = fallback_resp.headers().clone();
                        let content_type = fallback_headers
                            .get("content-type")
                            .and_then(|ct| ct.to_str().ok())
                            .unwrap_or("image/jpeg")
                            .to_string();

                        match fallback_resp.bytes().await {
                            Ok(bytes) => {
                                let current_time = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs();

                                let mut cache = THUMBNAIL_CACHE.lock().await;
                                cache.put(
                                    cache_key,
                                    (bytes.to_vec(), content_type.clone(), current_time),
                                );

                                HttpResponse::Ok()
                                    .content_type(content_type.as_str())
                                    .body(bytes)
                            }
                            Err(_) => HttpResponse::NotFound().finish(),
                        }
                    }
                    Err(_) => HttpResponse::NotFound().finish(),
                }
            } else {
                let content_type = headers
                    .get("content-type")
                    .and_then(|ct| ct.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();

                match resp.bytes().await {
                    Ok(bytes) => {
                        let current_time = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs();

                        let mut cache = THUMBNAIL_CACHE.lock().await;
                        cache.put(
                            cache_key,
                            (bytes.to_vec(), content_type.clone(), current_time),
                        );

                        HttpResponse::Ok()
                            .content_type(content_type.as_str())
                            .body(bytes)
                    }
                    Err(_) => HttpResponse::NotFound().finish(),
                }
            }
        }
        Err(_) => HttpResponse::NotFound().finish(),
    }
}

#[utoipa::path(
    get,
    path = "/channel_icon/{path_video_id}",
    params(
        ("path_video_id" = String, Path, description = "Channel ID (UC...), @handle, video ID or direct image URL")
    ),
    responses(
        (status = 200, description = "Channel icon image", content_type = "image/jpeg, image/png, image/webp"),
        (status = 404, description = "Channel icon not found"),
        (status = 400, description = "Bad request")
    )
)]
pub async fn channel_icon(
    path: web::Path<String>,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let input = path.into_inner();
    let config = &data.config;

    let decoded = urlencoding::decode(&input)
        .unwrap_or_else(|_| std::borrow::Cow::Owned(input.clone()))
        .to_string();
    
    if decoded.starts_with("http://") || decoded.starts_with("https://") {
        return proxy_image(&decoded).await;
    }

    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .build()
        .unwrap();

    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };

    let ctx = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20250130.08.00",
            "hl": "en",
            "gl": "US"
        }
    });

    let mut channel_id = String::new();

    if input.len() == 24 && input.starts_with("UC") {
        channel_id = input.clone();
    } else if input.starts_with('@') {
        let handle = &input[1..];
        let page_url = format!("https://www.youtube.com/@{}", handle);

        if let Ok(resp) = client.get(&page_url).send().await {
            if let Ok(html) = resp.text().await {
                if let Some(start) = html.find(r#""channelId":"UC"#) {
                    let slice = &html[start + 13..]; // после "channelId":"
                    if let Some(end) = slice.find('"') {
                        channel_id = slice[..end].to_string();
                    }
                }
                if channel_id.is_empty() {
                    if let Some(pos) = html.find(r#"<link rel="canonical" href="https://www.youtube.com/channel/"#) {
                        let slice = &html[pos + 47..]; // длина префикса
                        if let Some(end) = slice.find('"') {
                            channel_id = slice[..end].to_string();
                        }
                    }
                }
            }
        }
    } else {
        channel_id = get_channel_id_from_video(&client, &input, &innertube_key, &ctx).await;
    }

    if channel_id.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Cannot determine channel ID"}));
    }

    let avatar_url = get_channel_avatar_url(&client, &channel_id, &innertube_key, &ctx).await;

    if avatar_url.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Channel avatar not found"}));
    }

    proxy_image(&avatar_url).await
}

#[utoipa::path(
    get,
    path = "/get-ytvideo-info.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Video quality"),
        ("proxy" = Option<String>, Query, description = "Use video proxy (true/false)")
    ),
    responses(
        (status = 200, description = "Video information", body = VideoInfoResponse),
        (status = 400, description = "Missing video ID"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_ytvideo_info(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let base_trimmed = base.trim_end_matches('/');

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
                "error": "ID видео не был передан."
            }));
        }
    };

    let _quality = query_params
        .get("quality")
        .map(|s| s.as_str())
        .unwrap_or(&config.video.default_quality);
    let proxy_param = query_params
        .get("proxy")
        .map(|s| s.to_lowercase())
        .unwrap_or("true".to_string());
    let _use_video_proxy = proxy_param != "false";

    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };

    let client = Client::new();
    
    let video_url = format!("https://www.youtube.com/watch?v={}", video_id);
    
    let html = match client.get(&video_url).send().await {
        Ok(resp) => match resp.text().await {
            Ok(text) => text,
            Err(e) => {
                crate::log::info!("Error fetching video page: {}", e);
                return HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": "Failed to fetch video page"
                }));
            }
        },
        Err(e) => {
            crate::log::info!("Error fetching video page: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch video page"
            }));
        }
    };
    
    let cfg = extract_ytcfg(&html);
    let pr = extract_initial_player_response(&html);
    let api_key = cfg.get("INNERTUBE_API_KEY").and_then(|v| v.as_str()).unwrap_or(innertube_key);
    let mut ctx = cfg.get("INNERTUBE_CONTEXT").cloned().unwrap_or_else(|| {
        serde_json::json!({
            "client": {
                "clientName": "WEB",
                "clientVersion": "2.20250101"
            }
        })
    });
    
    if let Some(client) = ctx.get_mut("client").and_then(|c| c.as_object_mut()) {
        client.insert("gl".to_string(), serde_json::Value::String("US".to_string())); // Set region to USA
        client.insert("hl".to_string(), serde_json::Value::String("en-US".to_string())); // Set language to English (USA)
    }
    
    let next_payload = serde_json::json!({
        "context": ctx,
        "videoId": video_id
    });
    
    let next_url = format!("https://www.youtube.com/youtubei/v1/next?key={}", api_key);
    
    let next_data = match client
        .post(&next_url)
        .header("Content-Type", "application/json")
        .json(&next_payload)
        .send()
        .await
    {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(data) => data,
            Err(e) => {
                crate::log::info!("Error parsing next response: {}", e);
                serde_json::Value::Null
            }
        },
        Err(e) => {
            crate::log::info!("Error calling next endpoint: {}", e);
            serde_json::Value::Null
        }
    };
    
    let comments_token = get_comments_token(&next_data);
    let mut cont_resp = serde_json::Value::Null;
    
    if let Some(token) = comments_token {
        let cont_payload = serde_json::json!({
            "context": ctx,
            "continuation": token
        });
        
        cont_resp = match client
            .post(&next_url)
            .header("Content-Type", "application/json")
            .json(&cont_payload)
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(data) => data,
                Err(e) => {
                    crate::log::info!("Error parsing continuation response: {}", e);
                    serde_json::Value::Null
                }
            },
            Err(e) => {
                crate::log::info!("Error calling continuation endpoint: {}", e);
                serde_json::Value::Null
            }
        };
    }
    
    let vd = pr.get("videoDetails").unwrap_or(&serde_json::Value::Null);
    let micro = pr
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"))
        .unwrap_or(&serde_json::Value::Null);
    
    let comments = if !cont_resp.is_null() {
        extract_comments(&cont_resp, base_trimmed)
    } else {
        extract_comments(&next_data, base_trimmed)
    };
    
    let likes = find_likes(&next_data);
    
    let comm_cnt = find_comments_count(&pr, &next_data);
    let subscriber_count = find_subscriber_count(&next_data);
    
    let mut title = String::new();
    let mut author = String::new();
    let mut description = String::new();
    let mut published_at = String::new();
    let mut views = String::new();
    let mut channel_id = String::new();
    let mut channel_thumbnail = String::new();
    let _duration = String::new();
    
    if let Some(contents) = next_data.get("contents") {
        if let Some(two_col) = contents.get("twoColumnWatchNextResults") {
            if let Some(results) = two_col.get("results") {
                if let Some(results_inner) = results.get("results") {
                    if let Some(contents_array) = results_inner.get("contents").and_then(|c| c.as_array()) {
                        if contents_array.len() > 1 {
                            if let Some(primary_info) = contents_array[0].get("videoPrimaryInfoRenderer") {
                                if let Some(title_val) = primary_info.get("title") {
                                    title = simplify_text(title_val);
                                }
                                
                                if let Some(date_text) = primary_info.get("dateText") {
                                    published_at = simplify_text(date_text);
                                }
                                
                                if let Some(view_count) = primary_info.get("viewCount") {
                                    if let Some(video_view_count) = view_count.get("videoViewCountRenderer") {
                                        if let Some(view_count_simple) = video_view_count.get("viewCount") {
                                            views = simplify_text(view_count_simple);
                                            views.retain(|c| c.is_ascii_digit());
                                        }
                                    }
                                }
                            }
                            
                            if let Some(secondary_info) = contents_array[1].get("videoSecondaryInfoRenderer") {
                                if let Some(attr_desc) = secondary_info.get("attributedDescription") {
                                    description = attr_desc.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                                }
                                
                                if let Some(owner) = secondary_info.get("owner").and_then(|o| o.get("videoOwnerRenderer")) {
                                    if let Some(title_val) = owner.get("title") {
                                        author = simplify_text(title_val);
                                    }
                                    
                                    if let Some(nav_endpoint) = owner.get("navigationEndpoint") {
                                        if let Some(browse_endpoint) = nav_endpoint.get("browseEndpoint") {
                                            channel_id = browse_endpoint.get("browseId").and_then(|b| b.as_str()).unwrap_or("").to_string();
                                        }
                                    }
                                    
                                    if let Some(thumbnails) = owner.get("thumbnail").and_then(|t| t.get("thumbnails")).and_then(|arr| arr.as_array()) {
                                        if !thumbnails.is_empty() {
                                            channel_thumbnail = thumbnails[0].get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
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
    
    if title.is_empty() {
        title = vd.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
    }
    if author.is_empty() {
        if let Some(author_val) = vd.get("author").and_then(|a| a.as_str()) {
            author = author_val.to_string();
        } else if let Some(owner_name) = micro.get("ownerChannelName").and_then(|n| n.as_str()) {
            author = owner_name.to_string();
        }
    }
    if description.is_empty() {
        if let Some(desc_val) = vd.get("shortDescription").and_then(|d| d.as_str()) {
            description = desc_val.to_string();
        } else if let Some(desc_val) = vd.get("description").and_then(|d| d.as_str()) {
            description = desc_val.to_string();
        }
    }
    if published_at.is_empty() {
        published_at = micro.get("publishDate").and_then(|p| p.as_str()).unwrap_or("").to_string();
    }
    if views.is_empty() {
        if let Some(view_str) = vd.get("viewCount").and_then(|v| v.as_str()) {
            views = view_str.chars().filter(|c| c.is_ascii_digit()).collect();
        }
    }
    if channel_id.is_empty() {
        channel_id = vd.get("channelId").and_then(|c| c.as_str()).unwrap_or("").to_string();
    }
    
    let duration = if let Some(length_seconds) = vd.get("lengthSeconds").and_then(|l| l.as_str()) {
        if let Ok(seconds) = length_seconds.parse::<u64>() {
            format!("PT{}M{}S", seconds / 60, seconds % 60)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    
    let final_video_url = if config.video.source == "direct" {
        format!(
            "{}/direct_url?video_id={}",
            base_trimmed, video_id
        )
    } else {
        "".to_string()
    };
    
    let _final_video_url_with_proxy = if config.proxy.video_proxy && !final_video_url.is_empty() {
        format!(
            "{}/video.proxy?url={}",
            base_trimmed,
            urlencoding::encode(&final_video_url)
        )
    } else {
        final_video_url.clone()
    };
    
    let response = VideoInfoResponse {
        title: sanitize_text(&title),
        author,
        subscriber_count,
        description,
        video_id: video_id.clone(),
        channel_custom_url: micro
            .get("ownerProfileUrl")
            .and_then(|url| url.as_str())
            .and_then(|url_str| {
                url_str.rsplit('/').next().map(|part| part.to_string())
            }),
        embed_url: format!("https://www.youtube.com/embed/{}", video_id),
        duration,
        published_at,
        likes: if !likes.is_empty() { Some(likes) } else { None },
        views: if !views.is_empty() { Some(views) } else { None },
        comment_count: if !comm_cnt.is_empty() { 
            Some(comm_cnt) 
        } else { 
            Some(comments.len().to_string()) 
        },
        comments,
        channel_thumbnail: if !channel_thumbnail.is_empty() {
            format!("{}/channel_icon/{}", base_trimmed, urlencoding::encode(&channel_thumbnail))
        } else if !channel_id.is_empty() {
            format!("{}/channel_icon/{}", base_trimmed, channel_id)
        } else {
            "".to_string()
        },
        thumbnail: format!("{}/thumbnail/{}", base_trimmed, video_id),
        video_url: final_video_url,
    };
    
    HttpResponse::Ok().json(response)
}

#[utoipa::path(
    get,
    path = "/get_related_videos.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("count" = Option<i32>, Query, description = "Number of related videos to return (default: 50)"),
        ("offset" = Option<i32>, Query, description = "Offset for pagination (default: 0)"),
        ("limit" = Option<i32>, Query, description = "Limit for pagination (default: 50)"),
        ("order" = Option<String>, Query, description = "Order of results (relevance, date, rating, viewCount, title) (default: relevance)"),
        ("token" = Option<String>, Query, description = "Refresh token for InnerTube recommendations")
    ),
    responses(
        (status = 200, description = "List of related videos", body = [RelatedVideo]),
        (status = 400, description = "Missing video ID"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_related_videos(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
) -> impl Responder {
    let config = &data.config;
    let base = base_url(&req, config);
    let base_trimmed = base.trim_end_matches('/');

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
                "error": "ID видео не был передан."
            }));
        }
    };

    let quality = query_params
        .get("quality")
        .map(|q| q.clone())
        .unwrap_or_else(|| config.video.default_quality.clone());

    let count_param: i32 = query_params
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(config.video.default_count as i32);

    let limit: i32 = query_params
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(count_param);

    let offset: i32 = query_params
        .get("offset")
        .and_then(|o| o.parse().ok())
        .unwrap_or(0);

    let desired_count = limit.max(20).min(100); // Target more videos like in Python script

    let client = Client::new();
    
    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };
    
    let context = serde_json::json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20260128.05.00"
        }
    });

    let watch_url = format!("https://www.youtube.com/watch?v={}", video_id);
    let headers_map = {
        let mut map = reqwest::header::HeaderMap::new();
        map.insert(reqwest::header::USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/121.0.0.0 Safari/537.36".parse().unwrap());
        map.insert(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse().unwrap());
        map.insert(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
        map
    };

    let html_response = match client
        .get(&watch_url)
        .headers(headers_map.clone())
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
    {
        Ok(resp) => resp.text().await.unwrap_or_default(),
        Err(e) => {
            crate::log::info!("Error fetching watch page: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch video page"
            }));
        }
    };

    let ytcfg = extract_ytcfg(&html_response);
    let api_key_from_cfg = ytcfg.get("INNERTUBE_API_KEY").and_then(|v| v.as_str()).unwrap_or(innertube_key);
    let context_from_cfg = ytcfg.get("INNERTUBE_CONTEXT").cloned().unwrap_or(context);

    let next_url = format!("https://www.youtube.com/youtubei/v1/next?key={}", api_key_from_cfg);
    let body = serde_json::json!({
        "context": context_from_cfg,
        "videoId": video_id
    });

    let next_response = match client
        .post(&next_url)
        .headers(headers_map.clone())
        .json(&body)
        .timeout(std::time::Duration::from_secs(25))
        .send()
        .await
    {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(json) => json,
            Err(e) => {
                crate::log::info!("Error parsing next response: {}", e);
                return HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": "Failed to parse response"
                }));
            }
        },
        Err(e) => {
            crate::log::info!("Error making next request: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch related videos"
            }));
        }
    };

    let mut related_videos = extract_related_videos_from_response(&next_response);
    let mut continuation = get_related_continuation(&next_response);
    
    let mut page = 1;
    while let Some(cont_token) = continuation {
        if related_videos.len() >= desired_count as usize || page >= 6 {
            break;
        }
        
        page += 1;
        tokio::time::sleep(tokio::time::Duration::from_millis(1200 + (page as u64 * 300))).await;
        
        let cont_body = serde_json::json!({
            "context": context_from_cfg,
            "continuation": cont_token
        });
        
        let cont_response = match client
            .post(&next_url)
            .headers(headers_map.clone())
            .json(&cont_body)
            .timeout(std::time::Duration::from_secs(25))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(json) => json,
                Err(_) => break,
            },
            Err(_) => break,
        };
        
        let new_videos = extract_related_videos_from_response(&cont_response);
        if new_videos.is_empty() {
            break;
        }
        
        related_videos.extend(new_videos);
        continuation = get_related_continuation(&cont_response);
    }

    let mut seen = std::collections::HashSet::new();
    let unique_videos: Vec<_> = related_videos
        .into_iter()
        .filter(|v| {
            if v.video_id == video_id || seen.contains(&v.video_id) {
                false
            } else {
                seen.insert(v.video_id.clone());
                true
            }
        })
        .collect();

    let start_index = offset as usize;
    let end_index = (offset + limit) as usize;
    let paginated_videos = if start_index < unique_videos.len() {
        let actual_end = std::cmp::min(end_index, unique_videos.len());
        &unique_videos[start_index..actual_end]
    } else {
        &[][..]
    };

    let mut result_videos: Vec<RelatedVideo> = Vec::new();
    for video in paginated_videos {
        let thumbnail = format!("{}/thumbnail/{}", base_trimmed, video.video_id);
        let color = dominant_color_from_url(&format!("{}/thumbnail/{}", base_trimmed, video.video_id)).await;
        let channel_thumbnail = format!("{}/channel_icon/{}", base_trimmed, video.video_id);
        
        let video_url = format!("{}/get-ytvideo-info.php?video_id={}&quality={}", 
            base_trimmed, video.video_id, quality);
        
        let final_url = if config.proxy.video_proxy {
            format!("{}/video.proxy?url={}", 
                base_trimmed, urlencoding::encode(&video_url))
        } else {
            video_url
        };

        result_videos.push(RelatedVideo {
            title: video.title.clone(),
            author: video.channel.clone(),
            video_id: video.video_id.clone(),
            views: video.views.clone(),
            published_at: video.published.clone(),
            thumbnail,
            channel_thumbnail,
            url: final_url,
            source: "innertube".to_string(),
            color,
        });
    }

    HttpResponse::Ok().json(result_videos)
}

#[utoipa::path(
    get,
    path = "/get-direct-video-url.php",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality")
    ),
    responses(
        (status = 200, description = "Direct URL for the video", body = DirectUrlResponse),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn get_direct_video_url(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
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

    let quality = query_params.get("quality").map(|q| q.as_str());
    match resolve_direct_stream_url(&video_id, quality, false, &data.config).await {
        Ok(url) => HttpResponse::Ok().json(DirectUrlResponse { video_url: url }),
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "Failed to resolve direct url",
            "details": e
        })),
    }
}

#[utoipa::path(
    get,
    path = "/direct_url",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality"),
        ("proxy" = Option<String>, Query, description = "Pass-through proxy (true/false)"),
        ("codec" = Option<String>, Query, description = "Video codec for optional conversion: mpeg4 or h263. If passed, quality will be 360p")
    ),
    responses(
        (status = 200, description = "Video stream"),
        (status = 400, description = "Missing video_id or invalid codec")
    )
)]
pub async fn direct_url(req: HttpRequest, data: web::Data<crate::AppState>) -> impl Responder {
    spawn_direct_url_cleanup_if_needed();

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
                "error": "video_id parameter is required"
            }));
        }
    };

    let codec = query_params.get("codec").map(|c| c.as_str());

	if let Some(codec_str) = codec {
		if codec_str != "mpeg4" && codec_str != "h263" {
			return HttpResponse::BadRequest().json(serde_json::json!({
				"error": "Unsupported codec",
				"details": format!("Codec '{}' is not supported. Available: mpeg4, h263", codec_str),
				"supported_codecs": ["mpeg4", "h263"]
			}));
		}

		let direct_url = match resolve_direct_stream_url(&video_id, Some("360"), false, &data.config).await {
			Ok(url) => url,
			Err(e) => {
				return HttpResponse::InternalServerError().json(serde_json::json!({
					"error": "Failed to resolve video url for conversion",
					"details": e
				}));
			}
		};

		let user_agent = data.config.get_innertube_user_agent();
		let permit = data.codec_semaphore.clone().acquire_owned().await.ok();
		return stream_converted_video(&direct_url, &user_agent, &video_id, codec_str, permit);
	}

    let hls_only = query_params.get("hls").map(|v| v == "true").unwrap_or(false);

    if hls_only {
        match get_hls_manifest_url(&video_id, &data.config).await {
            Ok(manifest_url) => {
                HttpResponse::Ok().json(serde_json::json!({
                    "hls_manifest_url": manifest_url,
                    "video_id": video_id,
                    "message": "HLS Master Manifest URL - use this for streams without quality selection"
                }))
            },
            Err(e) => {
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": "Failed to get HLS manifest URL",
                    "details": e
                }))
            }
        }
    } else {
        let quality = query_params.get("quality").map(|q| q.as_str());
        let proxy_param = query_params
            .get("proxy")
            .map(|p| p.to_lowercase())
            .unwrap_or_else(|| "true".to_string());
        let use_proxy = proxy_param != "false";

        let use_quality_hls = quality.and_then(|q| parse_quality_height(q)).is_some();
        if use_quality_hls {
            let height = quality.and_then(|q| parse_quality_height(q)).unwrap();
            match fetch_player_response(&video_id, &data.config).await {
                Ok(player_data) => {
                    if let Ok((master_url, duration_seconds)) =
                        get_hls_manifest_url_and_duration_from_player(&player_data)
                    {
                        let ua = data.config.get_innertube_user_agent();
                        if let Ok(master_body) = fetch_hls_master_body(&master_url, &ua).await {
                            let audio_groups = parse_hls_audio_groups(&master_body, &master_url);
                            let variants =
                                parse_hls_master_variants(&master_body, &master_url, &audio_groups);
                            if let Some((video_url, audio_url)) =
                                pick_hls_variant_for_height(&variants, height)
                            {
                                let cache_path = hls_merge_cache_path(&video_id, height);
                                if !cache_path.exists() {
                                    let v_url = video_url.clone();
                                    let a_url = audio_url.clone();
                                    let c_path = cache_path.clone();
                                    let ua = data.config.get_innertube_user_agent().to_string();
                                    match task::spawn_blocking(move || {
                                        run_ffmpeg_hls_to_file(v_url, a_url, c_path, ua)
                                    })
                                    .await
                                    {
                                        Ok(Ok(())) => {}
                                        Ok(Err(e)) => {
                                            return HttpResponse::InternalServerError().json(
                                                serde_json::json!({
                                                    "error": "FFmpeg HLS merge failed",
                                                    "details": e
                                                }),
                                            );
                                        }
                                        Err(e) => {
                                            return HttpResponse::InternalServerError().json(
                                                serde_json::json!({
                                                    "error": "Task join error",
                                                    "details": e.to_string()
                                                }),
                                            );
                                        }
                                    }
                                }
                                return serve_mp4_from_cache(
                                    &cache_path,
                                    &req,
                                    duration_seconds,
                                );
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }

        let direct_url = if quality.is_none() {
            fetch_player_response(&video_id, &data.config)
                .await
                .ok()
                .and_then(|data| get_direct_stream_url_from_player_response(&data))
        } else {
            None
        };
        let direct_url = match direct_url {
            Some(url) => url,
            None => match resolve_direct_stream_url(&video_id, quality, false, &data.config).await {
                Ok(url) => url,
                Err(e) => {
                    return HttpResponse::InternalServerError().json(serde_json::json!({
                        "error": "Failed to resolve video url",
                        "details": e
                    }));
                }
            }
        };

        if req.method() == actix_web::http::Method::HEAD {
            let client = Client::new();
            match client.head(&direct_url).send().await {
                Ok(resp) => {
                    let mut builder = HttpResponse::build(resp.status());
                    if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                        builder.insert_header((CONTENT_LENGTH, len.clone()));
                    }
                    if let Some(range) = resp.headers().get(CONTENT_RANGE) {
                        builder.insert_header((CONTENT_RANGE, range.clone()));
                    }
                    builder.insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")));
                    builder.finish()
                }
                Err(_) => HttpResponse::Ok().finish(),
            }
        } else if !use_proxy {
            HttpResponse::Found()
                .insert_header((LOCATION, direct_url))
                .finish()
        } else {
            proxy_stream_response(&direct_url, &req, "video/mp4").await
        }
    }
}

#[utoipa::path(
    get,
    path = "/hls_manifest_url",
    params(
        ("video_id" = String, Query, description = "YouTube video ID")
    ),
    responses(
        (status = 200, description = "HLS Manifest URL", body = HlsManifestUrlResponse),
        (status = 400, description = "Missing video_id"),
        (status = 500, description = "Failed to get manifest URL")
    )
)]
pub async fn hls_manifest_url(req: HttpRequest, data: web::Data<crate::AppState>) -> impl Responder {
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
                "error": "video_id parameter is required"
            }));
        }
    };

    match get_hls_manifest_url(&video_id, &data.config).await {
        Ok(manifest_url) => {
            HttpResponse::Ok().json(HlsManifestUrlResponse {
                hls_manifest_url: manifest_url,
                video_id,
                message: Some("HLS Master Manifest URL - use this for streams without quality selection".to_string()),
            })
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to get HLS manifest URL",
                "details": e
            }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/direct_audio_url",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("proxy" = Option<String>, Query, description = "Pass-through proxy (true/false)")
    ),
    responses(
        (status = 200, description = "Audio stream"),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn direct_audio_url(
    req: HttpRequest,
    data: web::Data<crate::AppState>,
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

    let proxy_param = query_params
        .get("proxy")
        .map(|p| p.to_lowercase())
        .unwrap_or_else(|| "true".to_string());
    let use_proxy = proxy_param != "false";

    let direct_url = match resolve_direct_stream_url(&video_id, None, true, &data.config).await {
        Ok(url) => url,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to resolve audio url",
                "details": e
            }));
        }
    };

    if req.method() == actix_web::http::Method::HEAD {
        let client = Client::new();
        match client.head(&direct_url).send().await {
            Ok(resp) => {
                let mut builder = HttpResponse::build(resp.status());
                if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                    builder.insert_header((CONTENT_LENGTH, len.clone()));
                }
                if let Some(range) = resp.headers().get(CONTENT_RANGE) {
                    builder.insert_header((CONTENT_RANGE, range.clone()));
                }
                builder.insert_header((CONTENT_TYPE, HeaderValue::from_static("audio/m4a")));
                builder.finish()
            }
            Err(_) => HttpResponse::Ok().finish(),
        }
    } else if !use_proxy {
        HttpResponse::Found()
            .insert_header((LOCATION, direct_url))
            .finish()
    } else {
        proxy_stream_response(&direct_url, &req, "audio/m4a").await
    }
}

#[utoipa::path(
    get,
    path = "/video.proxy",
    params(
        ("url" = String, Query, description = "Target URL to proxy")
    ),
    responses(
        (status = 200, description = "Proxied response")
    )
)]
pub async fn video_proxy(req: HttpRequest) -> impl Responder {
    let mut query_params: HashMap<String, String> = HashMap::new();
    for pair in req.query_string().split('&') {
        let mut parts = pair.split('=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            query_params.insert(key.to_string(), value.to_string());
        }
    }

    let url = match query_params.get("url") {
        Some(u) => urlencoding::decode(u)
            .unwrap_or_else(|_| u.into())
            .to_string(),
        None => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Missing url parameter"
            }));
        }
    };

    if req.method() == actix_web::http::Method::HEAD {
        let client = Client::new();
        match client.head(&url).send().await {
            Ok(resp) => {
                let mut builder = HttpResponse::build(resp.status());
                if let Some(len) = resp.headers().get(CONTENT_LENGTH) {
                    builder.insert_header((CONTENT_LENGTH, len.clone()));
                }
                if let Some(ct) = resp.headers().get(CONTENT_TYPE) {
                    builder.insert_header((CONTENT_TYPE, ct.clone()));
                }
                builder.finish()
            }
            Err(_) => HttpResponse::Ok().finish(),
        }
    } else {
        proxy_stream_response(&url, &req, "application/octet-stream").await
    }
}

#[utoipa::path(
    get,
    path = "/download",
    params(
        ("video_id" = String, Query, description = "YouTube video ID"),
        ("quality" = Option<String>, Query, description = "Preferred quality")
    ),
    responses(
        (status = 302, description = "Redirect to downloadable stream")
    )
)]
pub async fn download_video(req: HttpRequest, data: web::Data<crate::AppState>) -> impl Responder {
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

    let quality = query_params.get("quality").map(|q| q.as_str());
    let direct_url = match resolve_direct_stream_url(&video_id, quality, false, &data.config).await
    {
        Ok(url) => url,
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to resolve video url",
                "details": e
            }));
        }
    };

    if req.method() == actix_web::http::Method::HEAD {
        HttpResponse::Ok().finish()
    } else {
        HttpResponse::Found()
            .insert_header((LOCATION, direct_url))
            .insert_header((
                "Content-Disposition",
                format!("attachment; filename=\"{}.mp4\"", video_id),
            ))
            .finish()
    }
}


fn get_related_continuation(data: &serde_json::Value) -> Option<String> {
    if let Some(contents) = data.get("contents")
        .and_then(|c| c.get("twoColumnWatchNextResults"))
        .and_then(|c| c.get("secondaryResults"))
        .and_then(|c| c.get("secondaryResults"))
        .and_then(|c| c.get("results"))
        .and_then(|c| c.as_array())
    {
        for item in contents {
            if let Some(item_section) = item.get("itemSectionRenderer") {
                if let Some(contents_arr) = item_section.get("contents").and_then(|c| c.as_array()) {
                    for content in contents_arr {
                        if let Some(cont_renderer) = content.get("continuationItemRenderer") {
                            if let Some(cont_endpoint) = cont_renderer
                                .get("continuationEndpoint")
                                .and_then(|ce| ce.get("continuationCommand"))
                            {
                                if let Some(token) = cont_endpoint.get("token").and_then(|t| t.as_str()) {
                                    return Some(token.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
struct RelatedVideoInfo {
    video_id: String,
    title: String,
    channel: String,
    views: String,
    duration: String,
    thumbnail: String,
    published: String,
}

fn extract_related_videos_from_response(data: &serde_json::Value) -> Vec<RelatedVideoInfo> {
    let mut videos = Vec::new();
    
    walk_json_for_videos(data, &mut videos);
    
    videos
}

fn walk_json_for_videos(obj: &serde_json::Value, videos: &mut Vec<RelatedVideoInfo>) {
    match obj {
        serde_json::Value::Object(map) => {
            if let Some(lockup_view_model) = map.get("lockupViewModel") {
                if let Some(video_info) = extract_video_from_lockup(lockup_view_model) {
                    videos.push(video_info);
                }
            }
            
            for (_, value) in map {
                walk_json_for_videos(value, videos);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                walk_json_for_videos(item, videos);
            }
        }
        _ => {}
    }
}

fn extract_video_from_lockup(lockup: &serde_json::Value) -> Option<RelatedVideoInfo> {
    let renderer_context = lockup.get("rendererContext")?.as_object()?;
    let command_context = renderer_context.get("commandContext")?.as_object()?;
    let on_tap = command_context.get("onTap")?.as_object()?;
    let innertube_command = on_tap.get("innertubeCommand")?.as_object()?;
    let watch_endpoint = innertube_command.get("watchEndpoint")?.as_object()?;
    
    let video_id = watch_endpoint.get("videoId")?.as_str()?.to_string();
    
    let metadata = lockup.get("metadata")?.as_object()?;
    let lockup_metadata = metadata.get("lockupMetadataViewModel")?.as_object()?;
    let title = lockup_metadata.get("title")?
        .as_object()?
        .get("content")?
        .as_str()?
        .to_string();
    
    let metadata_rows = metadata
        .get("lockupMetadataViewModel")?
        .as_object()?
        .get("metadata")?
        .as_object()?
        .get("contentMetadataViewModel")?
        .as_object()?
        .get("metadataRows")?
        .as_array()?
        .to_vec();
    
    let mut channel = "—".to_string();
    let mut views = "".to_string();
    let mut published = "—".to_string();
    
    if !metadata_rows.is_empty() {
        if let Some(first_row) = metadata_rows.first() {
            if let Some(metadata_parts) = first_row.as_object()
                .and_then(|r| r.get("metadataParts"))
                .and_then(|p| p.as_array()) 
            {
                if let Some(first_part) = metadata_parts.first() {
                    if let Some(text_content) = first_part.as_object()
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_object())
                        .and_then(|t| t.get("content"))
                        .and_then(|c| c.as_str())
                    {
                        channel = text_content.trim().to_string();
                    }
                }
            }
        }
        
        if metadata_rows.len() > 1 {
            if let Some(second_row) = metadata_rows.get(1) {
                if let Some(metadata_parts) = second_row.as_object()
                    .and_then(|r| r.get("metadataParts"))
                    .and_then(|p| p.as_array())
                {
                    if metadata_parts.len() >= 1 {
                        if let Some(views_raw) = metadata_parts[0].as_object()
                            .and_then(|p| p.get("text"))
                            .and_then(|t| t.as_object())
                            .and_then(|t| t.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            views = clean_views_string(views_raw);
                        }
                    }
                    
                    if metadata_parts.len() >= 2 {
                        if let Some(published_raw) = metadata_parts[1].as_object()
                            .and_then(|p| p.get("text"))
                            .and_then(|t| t.as_object())
                            .and_then(|t| t.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            published = published_raw.trim().to_string();
                        }
                    }
                }
            }
        }
    }
    
    let mut duration = "—".to_string();
    if let Some(content_image) = lockup.get("contentImage").and_then(|ci| ci.as_object()) {
        if let Some(thumbnail_vm) = content_image.get("thumbnailViewModel").and_then(|tvm| tvm.as_object()) {
            if let Some(overlays) = thumbnail_vm.get("overlays").and_then(|o| o.as_array()) {
                for overlay in overlays {
                    if let Some(badge_vm) = overlay.as_object()
                        .and_then(|o| o.get("thumbnailOverlayBadgeViewModel"))
                        .and_then(|bvm| bvm.as_object())
                    {
                        if let Some(thumbnail_badges) = badge_vm.get("thumbnailBadges").and_then(|tb| tb.as_array()) {
                            if let Some(first_badge) = thumbnail_badges.first() {
                                if let Some(badge_text) = first_badge.as_object()
                                    .and_then(|b| b.get("thumbnailBadgeViewModel"))
                                    .and_then(|tbm| tbm.as_object())
                                    .and_then(|tbm| tbm.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    duration = badge_text.to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    let thumbnail = String::new();
    
    Some(RelatedVideoInfo {
        video_id,
        title,
        channel,
        views,
        duration,
        thumbnail,
        published,
    })
}


async fn fetch_player_response(
    video_id: &str,
    config: &crate::config::Config,
) -> Result<Value, String> {
    let api_key = config
        .get_innertube_key()
        .ok_or("innertube api key не задан в config.yml (api.innertube.key)")?;
    let client = Client::new();
    let user_agent = config.get_innertube_user_agent();
    let player_client = config.get_innertube_player_client();
    let json_data = serde_json::json!({
        "context": {
            "client": player_client.to_player_context_value()
        },
        "videoId": video_id
    });
    let url = format!("https://www.youtube.com/youtubei/v1/player?key={}", api_key);
    let resp = client
        .post(&url)
        .header("User-Agent", &user_agent)
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/json")
        .json(&json_data)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("player API HTTP {}", resp.status()));
    }
    resp.json::<Value>().await.map_err(|e| e.to_string())
}

async fn get_hls_manifest_url(video_id: &str, config: &crate::config::Config) -> Result<String, String> {
    let data = fetch_player_response(video_id, config).await?;
    get_hls_manifest_url_from_player(&data)
}

fn get_hls_manifest_url_and_duration_from_player(data: &Value) -> Result<(String, Option<u64>), String> {
    let streaming_data = data
        .get("streamingData")
        .ok_or("streamingData отсутствует")?;
    let hls = streaming_data
        .get("hlsManifestUrl")
        .and_then(|v| v.as_str())
        .ok_or("hlsManifestUrl отсутствует (приватное/возраст/регион)")?;

    let duration_seconds = streaming_data
        .get("formats")
        .and_then(|a| a.get(0))
        .and_then(|f| f.get("approxDurationMs"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| (ms + 999) / 1000)
        .or_else(|| {
            streaming_data
                .get("adaptiveFormats")
                .and_then(|a| a.get(0))
                .and_then(|f| f.get("approxDurationMs"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
                .map(|ms| (ms + 999) / 1000)
        })
        .or_else(|| {
            data.get("videoDetails")
                .and_then(|vd| vd.get("lengthSeconds"))
                .and_then(|l| l.as_str())
                .and_then(|s| s.parse::<u64>().ok())
        });

    Ok((hls.to_string(), duration_seconds))
}

fn get_hls_manifest_url_from_player(data: &Value) -> Result<String, String> {
    get_hls_manifest_url_and_duration_from_player(data).map(|(url, _)| url)
}

fn get_direct_stream_url_from_player_response(data: &Value) -> Option<String> {
    let streaming = data.get("streamingData")?;
    let mut best: Option<(u32, &str)> = None;
    for key in &["formats", "adaptiveFormats"] {
        let arr = streaming.get(*key)?.as_array()?;
        for f in arr {
            let url = f.get("url").and_then(|v| v.as_str())?;
            let label = f.get("qualityLabel").and_then(|v| v.as_str()).unwrap_or("");
            let height: u32 = label.trim_end_matches('p').parse().unwrap_or(0);
            if *key == "adaptiveFormats" && height == 0 {
                continue;
            }
            let replace = match best {
                None => true,
                Some((h, _)) => height > h,
            };
            if replace {
                best = Some((height, url));
            }
        }
    }
    best.map(|(_, u)| u.to_string())
}

async fn fetch_hls_master_body(master_url: &str, user_agent: &str) -> Result<String, String> {
    let client = Client::builder()
        .user_agent(user_agent)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(master_url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Master manifest HTTP {}", resp.status()));
    }
    resp.text().await.map_err(|e| e.to_string())
}

fn parse_hls_audio_groups(master_body: &str, master_base_url: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let re_group = regex::Regex::new(r#"GROUP-ID="([^"]+)""#).unwrap();
    let re_uri = regex::Regex::new(r#"URI="([^"]+)""#).unwrap();
    for line in master_body.lines().map(str::trim) {
        if line.starts_with("#EXT-X-MEDIA:") && line.contains("TYPE=AUDIO") {
            let group_id = re_group.captures(line).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
            let uri = re_uri.captures(line).and_then(|c| c.get(1)).map(|m| m.as_str());
            if let (Some(g), Some(u)) = (group_id, uri) {
                let full = if u.starts_with("http") {
                    u.to_string()
                } else {
                    let base = master_base_url.rsplit_once('/').map(|(b, _)| b).unwrap_or(master_base_url);
                    format!("{}/{}", base, u)
                };
                if !map.contains_key(&g) {
                    map.insert(g, full);
                }
            }
        }
    }
    map
}

fn parse_hls_master_variants(
    master_body: &str,
    master_base_url: &str,
    audio_groups: &HashMap<String, String>,
) -> Vec<(u32, String, Option<String>)> {
    let re_res = regex::Regex::new(r"RESOLUTION=(\d+)x(\d+)").unwrap();
    let re_audio = regex::Regex::new(r#"AUDIO="([^"]+)""#).unwrap();
    let mut variants = Vec::new();
    let lines: Vec<&str> = master_body.lines().map(str::trim).collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("#EXT-X-STREAM-INF:") {
            let height = re_res.captures(line).and_then(|c| c.get(2)).and_then(|m| m.as_str().parse::<u32>().ok());
            let audio_group = re_audio.captures(line).and_then(|c| c.get(1)).map(|m| m.as_str());
            i += 1;
            if i < lines.len() {
                let uri = lines[i].trim();
                if !uri.is_empty() && !uri.starts_with('#') {
                    let video_full = if uri.starts_with("http") {
                        uri.to_string()
                    } else {
                        let base = master_base_url.rsplit_once('/').map(|(b, _)| b).unwrap_or(master_base_url);
                        format!("{}/{}", base, uri)
                    };
                    let audio_url = audio_group.and_then(|g| audio_groups.get(g)).cloned();
                    if let Some(h) = height {
                        variants.push((h, video_full, audio_url));
                    }
                }
            }
        }
        i += 1;
    }
    variants
}

fn hls_merge_cache_path(video_id: &str, height: u32) -> PathBuf {
    let dir = env::temp_dir().join("yt_api_hls_cache");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{}_{}.mp4", video_id, height))
}

fn run_ffmpeg_hls_to_file(
    video_url: String,
    audio_url: Option<String>,
    cache_path: PathBuf,
    user_agent: String,
) -> Result<(), String> {
    let headers_arg = "Referer: https://www.youtube.com\r\nOrigin: https://www.youtube.com";
    let tmp_path = cache_path.with_extension("tmp");
    let out_path = tmp_path.to_string_lossy().to_string();

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-reconnect".into(),
        "1".into(),
        "-reconnect_streamed".into(),
        "1".into(),
        "-reconnect_at_eof".into(),
        "1".into(),
        "-reconnect_delay_max".into(),
        "10".into(),
        "-user_agent".into(),
        user_agent.clone(),
        "-headers".into(),
        headers_arg.to_string(),
        "-i".into(),
        video_url.clone(),
    ];
    if let Some(ref audio) = audio_url {
        args.extend([
            "-user_agent".into(),
            user_agent,
            "-headers".into(),
            headers_arg.to_string(),
            "-i".into(),
            audio.clone(),
        ]);
    }
    if audio_url.is_some() {
        args.extend([
            "-map".into(),
            "0:v:0".into(),
            "-map".into(),
            "1:a:0".into(),
            "-c:v".into(),
            "copy".into(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "160k".into(),
        ]);
    } else {
        args.extend(["-c".into(), "copy".into()]);
    }
    args.extend([
        "-movflags".into(),
        "frag_keyframe+empty_moov".into(),
        "-f".into(),
        "mp4".into(),
        out_path,
    ]);

    let status = Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ffmpeg spawn: {}", e))?
        .wait()
        .map_err(|e| format!("ffmpeg wait: {}", e))?;
    if !status.success() {
        return Err(format!("ffmpeg exit: {}", status));
    }
    fs::rename(&tmp_path, &cache_path).map_err(|e| format!("rename cache: {}", e))?;
    Ok(())
}

fn serve_mp4_from_cache(
    path: &Path,
    req: &HttpRequest,
    duration_seconds: Option<u64>,
) -> HttpResponse {
    let file_size = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return HttpResponse::NotFound().finish(),
    };
    if req.method() == actix_web::http::Method::HEAD {
        let mut builder = HttpResponse::Ok();
        builder
            .insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")))
            .insert_header(("Accept-Ranges", "bytes"))
            .insert_header((CONTENT_LENGTH, file_size.to_string()));
        if let Some(secs) = duration_seconds {
            let s = secs.to_string();
            builder
                .insert_header(("X-Content-Duration", s.as_str()))
                .insert_header(("Content-Duration", s.as_str()))
                .insert_header(("X-Video-Duration", s.as_str()))
                .insert_header(("X-Duration-Seconds", s.as_str()));
        }
        return builder.finish();
    }
    let range_header = req.headers().get("Range").and_then(|v| v.to_str().ok());
    let (start, end, status, content_range) = if let Some(range) = range_header {
        let mut start = 0u64;
        let mut end = file_size.saturating_sub(1);
        if let Some(cap) = regex::Regex::new(r"bytes=(\d+)-(\d*)").ok().and_then(|r| r.captures(range)) {
            if let Some(s) = cap.get(1).and_then(|m| m.as_str().parse::<u64>().ok()) {
                start = s.min(file_size.saturating_sub(1));
            }
            if let Some(m) = cap.get(2).map(|m| m.as_str()) {
                if !m.is_empty() {
                    if let Ok(e) = m.parse::<u64>() {
                        end = e.min(file_size.saturating_sub(1));
                    }
                }
            }
        }
        let content_range_val = format!("bytes {}-{}/{}", start, end, file_size);
        (start, end, actix_web::http::StatusCode::PARTIAL_CONTENT, Some(content_range_val))
    } else {
        (0, file_size.saturating_sub(1), actix_web::http::StatusCode::OK, None)
    };
    let start = start;
    let end = end;
    let content_range = content_range;
    let body = match fs::File::open(path) {
        Ok(mut f) => {
            let _ = f.seek(std::io::SeekFrom::Start(start));
            let len = end.saturating_sub(start) + 1;
            let mut buf = vec![0u8; len as usize];
            if let Ok(n) = f.read(&mut buf) {
                buf.truncate(n);
            }
            buf
        }
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };
    let mut builder = HttpResponse::build(status);
    builder
        .insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")))
        .insert_header(("Accept-Ranges", "bytes"))
        .insert_header((CONTENT_LENGTH, body.len()));
    if let Some(cr) = content_range {
        builder.insert_header((CONTENT_RANGE, cr));
    }
    if let Some(secs) = duration_seconds {
        let s = secs.to_string();
        builder
            .insert_header(("X-Content-Duration", s.as_str()))
            .insert_header(("Content-Duration", s.as_str()))
            .insert_header(("X-Video-Duration", s.as_str()))
            .insert_header(("X-Duration-Seconds", s.as_str()));
    }
    builder.body(body)
}

fn pick_hls_variant_for_height(
    variants: &[(u32, String, Option<String>)],
    requested_height: u32,
) -> Option<(String, Option<String>)> {
    let exact = variants.iter().find(|(h, _, _)| *h == requested_height);
    let chosen = exact.or_else(|| {
        variants
            .iter()
            .filter(|(h, _, _)| *h <= requested_height)
            .max_by_key(|(h, _, _)| *h)
    })?;
    Some((chosen.1.clone(), chosen.2.clone()))
}

fn stream_hls_to_mp4_response(
    video_playlist_url: &str,
    audio_playlist_url: Option<&str>,
    user_agent: &str,
    duration_seconds: Option<u64>,
    cache_path: Option<PathBuf>,
) -> HttpResponse {
    let video_url = video_playlist_url.to_string();
    let audio_url = audio_playlist_url.map(String::from);
    let user_agent = user_agent.to_string();
    let headers_arg = "Referer: https://www.youtube.com\r\nOrigin: https://www.youtube.com";
    let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(8);
    std::thread::spawn(move || {
        let args: Vec<&str> = match &audio_url {
            None => vec![
                "-hide_banner", "-loglevel", "error", "-nostdin",
                "-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_at_eof", "1", "-reconnect_delay_max", "10",
                "-user_agent", &user_agent, "-headers", headers_arg,
                "-i", &video_url,
                "-c", "copy",
                "-movflags", "frag_keyframe+empty_moov",
                "-f", "mp4", "-",
            ],
            Some(audio) => vec![
                "-hide_banner", "-loglevel", "error", "-nostdin",
                "-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_at_eof", "1", "-reconnect_delay_max", "10",
                "-user_agent", &user_agent, "-headers", headers_arg, "-i", &video_url,
                "-user_agent", &user_agent, "-headers", headers_arg, "-i", audio,
                "-map", "0:v:0", "-map", "1:a:0",
                "-c:v", "copy", "-c:a", "aac", "-b:a", "160k",
                "-movflags", "frag_keyframe+empty_moov",
                "-f", "mp4", "-",
            ],
        };
        let mut child = match Command::new("ffmpeg").args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.try_send(Err(std::io::Error::new(std::io::ErrorKind::Other, format!("ffmpeg spawn: {}", e))));
                return;
            }
        };
        if let Some(mut stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let mut line = String::new();
                let mut buf = [0u8; 512];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    for &b in &buf[..n] {
                        if b == b'\n' || b == b'\r' {
                            if !line.is_empty() {
                                line.clear();
                            }
                        } else {
                            line.push(b as char);
                        }
                    }
                }
                if !line.is_empty() {
                }
            });
        }
        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => return,
        };
        let mut cache_file: Option<fs::File> = cache_path.as_ref().and_then(|p| {
            let tmp = p.with_extension("tmp");
            fs::File::create(&tmp).ok()
        });
        const CHUNK: usize = 65536;
        let mut buf = [0u8; CHUNK];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = Bytes::from(buf[..n].to_vec());
                    if let Some(ref mut f) = cache_file {
                        let _ = f.write_all(&chunk);
                    }
                    if tx.blocking_send(Ok(chunk)).is_err() {
                        let _ = child.kill();
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.try_send(Err(e));
                    let _ = child.kill();
                    break;
                }
            }
        }
        if let (Some(f), Some(p)) = (cache_file, cache_path) {
            let _ = f.sync_all();
            drop(f);
            let tmp = p.with_extension("tmp");
            let _ = fs::rename(&tmp, &p);
        }
    });
    let stream = ReceiverStream::new(rx)
        .map(|r| r.map(web::Bytes::from).map_err(actix_web::error::ErrorInternalServerError));
    let mut builder = HttpResponse::Ok();
    builder
        .insert_header((CONTENT_TYPE, HeaderValue::from_static("video/mp4")))
        .insert_header(("Accept-Ranges", "bytes"))
        .insert_header(("Cache-Control", "public, max-age=3600"));
    if let Some(secs) = duration_seconds {
        let s = secs.to_string();
        builder
            .insert_header(("X-Content-Duration", s.as_str()))
            .insert_header(("Content-Duration", s.as_str()))
            .insert_header(("X-Video-Duration", s.as_str()))
            .insert_header(("X-Duration-Seconds", s.as_str()));
    }
    builder.streaming(stream)
}

fn stream_ffmpeg_response(
    rx: tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
    mime_type: &str,
) -> HttpResponse {
    let stream = ReceiverStream::new(rx)
        .map(|r| r.map(web::Bytes::from).map_err(actix_web::error::ErrorInternalServerError));
    HttpResponse::Ok()
        .insert_header((CONTENT_TYPE, HeaderValue::from_str(mime_type).unwrap()))
        .insert_header(("Cache-Control", "public, max-age=3600"))
        .streaming(stream)
}

async fn get_channel_id_from_video(
    client: &Client,
    video_id: &str,
    key: &str,
    ctx: &serde_json::Value,
) -> String {
    let url = format!("https://www.youtube.com/youtubei/v1/player?key={}", key);

    let payload = serde_json::json!({
        "context": ctx,
        "videoId": video_id
    });

    match client
        .post(&url)
        .json(&payload)
        .header("Content-Type", "application/json")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json.pointer("/videoDetails/channelId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

async fn get_channel_avatar_url(
    client: &Client,
    channel_id: &str,
    key: &str,
    ctx: &serde_json::Value,
) -> String {
    let url = format!("https://www.youtube.com/youtubei/v1/browse?key={}", key);

    let payload = serde_json::json!({
        "context": ctx,
        "browseId": channel_id
    });

    match client
        .post(&url)
        .json(&payload)
        .header("Content-Type", "application/json")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(header) = json.pointer("/header/c4TabbedHeaderRenderer") {
                    if let Some(thumbs) = header
                        .pointer("/avatar/thumbnails")
                        .and_then(|arr| arr.as_array())
                    {
                        if let Some(best) = thumbs.iter().max_by_key(|t| {
                            let w = t.pointer("/width").and_then(|w| w.as_u64()).unwrap_or(0);
                            w
                        }) {
                            if let Some(u) = best.pointer("/url").and_then(|u| u.as_str()) {
                                let mut url = u.to_string();
                                if url.contains("yt3.ggpht.com") {
                                    url = url.replace("yt3.ggpht.com", "yt3.googleusercontent.com");
                                }
                                return url;
                            }
                        }
                    }
                }

                json.pointer("/metadata/channelMetadataRenderer/avatar/thumbnails")
                    .and_then(|arr| arr.as_array())
                    .and_then(|thumbs| thumbs.last())
                    .and_then(|t| t.get("url").and_then(|u| u.as_str()))
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

async fn proxy_image(url: &str) -> HttpResponse {
    let processed_url = url.replace("s900", "s88");
    
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .build()
        .unwrap();

    match client.get(&processed_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_string();

            match resp.bytes().await {
                Ok(bytes) => HttpResponse::Ok()
                    .content_type(content_type)
                    .insert_header(("Cache-Control", "public, max-age=86400"))
                    .body(bytes),
                Err(_) => HttpResponse::NotFound().finish(),
            }
        }
        _ => HttpResponse::NotFound().finish(),
    }
}

fn clean_views_string(views_raw: &str) -> String {
    let cleaned = views_raw.replace(|c: char| !c.is_ascii_digit() && c != 'K' && c != 'M' && c != '.', "");
    cleaned.replace("K", "000").replace("M", "000000").replace(".", "")
}