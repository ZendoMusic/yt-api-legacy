use actix_web::http::header::{HeaderValue, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION};
use actix_web::{web, HttpRequest, HttpResponse, Responder};
use futures_util::StreamExt;
use html_escape::decode_html_entities;
use image::{GenericImageView, Pixel};
use lazy_static::lazy_static;
use lru::LruCache;
use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
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
    // Try multiple patterns like in the Python script
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
    // Try to find the comment section and extract continuation token
    // This mimics the Python logic
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
                // Check if this is the comment section
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
        // More efficient string collection without intermediate vector
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
    // Follow the exact Python script logic
    // Navigate to: next_data["contents"]["twoColumnWatchNextResults"]["results"]["results"]["contents"][0]["videoPrimaryInfoRenderer"]["videoActions"]["menuRenderer"]["topLevelButtons"][0]["segmentedLikeDislikeButtonViewModel"]
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
                                    // Now navigate to: likeButtonViewModel.likeButtonViewModel.toggleButtonViewModel.toggleButtonViewModel
                                    if let Some(like_button_vm) = button.get("likeButtonViewModel") {
                                        if let Some(like_button_vm2) = like_button_vm.get("likeButtonViewModel") {
                                            if let Some(toggle_button_vm) = like_button_vm2.get("toggleButtonViewModel") {
                                                if let Some(toggle_button_vm2) = toggle_button_vm.get("toggleButtonViewModel") {
                                                    // 1. Try toggledButtonViewModel (like in Python script)
                                                    if let Some(toggled_btn) = toggle_button_vm2.get("toggledButtonViewModel") {
                                                        if let Some(button_vm) = toggled_btn.get("buttonViewModel",) {
                                                            if let Some(title) = button_vm.get("title").and_then(|t| t.as_str()) {
                                                                if !title.is_empty() && title.chars().any(|c| c.is_ascii_digit()) {
                                                                    println!("DEBUG: взято из toggled.title = {}", title);
                                                                    return parse_human_number(title);
                                                                }
                                                            }
                                                            
                                                            // Also try accessibilityText from toggled button
                                                            if let Some(acc_text) = button_vm.get("accessibilityText").and_then(|t| t.as_str()) {
                                                                if !acc_text.is_empty() {
                                                                    // Try pattern "along with X other"
                                                                    if let Some(caps) = regex::Regex::new(r"along with ([\d, ]*) other").unwrap().captures(acc_text) {
                                                                        let num = caps[1].replace(",", "").replace(" ", "");
                                                                        println!("DEBUG: взято из accessibility = {}", num);
                                                                        return num;
                                                                    }
                                                                    // Try general digit pattern
                                                                    if let Some(caps) = regex::Regex::new(r"(\d[\d, ]*)").unwrap().captures(acc_text) {
                                                                        return parse_human_number(&caps[1]);
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    
                                                    // 2. Try defaultButtonViewModel (like in Python script)
                                                    if let Some(default_btn) = toggle_button_vm2.get("defaultButtonViewModel") {
                                                        if let Some(button_vm) = default_btn.get("buttonViewModel") {
                                                            if let Some(title) = button_vm.get("title").and_then(|t| t.as_str()) {
                                                                if !title.is_empty() && title.chars().any(|c| c.is_ascii_digit()) {
                                                                    println!("DEBUG: взято из default.title = {}", title);
                                                                    return parse_human_number(title);
                                                                }
                                                            }
                                                            
                                                            // Also try accessibilityText from default button
                                                            if let Some(acc_text) = button_vm.get("accessibilityText").and_then(|t| t.as_str()) {
                                                                if !acc_text.is_empty() {
                                                                    // Try pattern "along with X other"
                                                                    if let Some(caps) = regex::Regex::new(r"along with ([\d, ]*) other").unwrap().captures(acc_text) {
                                                                        let num = caps[1].replace(",", "").replace(" ", "");
                                                                        println!("DEBUG: взято из accessibility = {}", num);
                                                                        return num;
                                                                    }
                                                                    // Try general digit pattern
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
    
    // Fallback to the old method
    if let Some(micro) = next_data
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"))
    {
        if let Some(like_count) = micro.get("likeCount").and_then(|lc| lc.as_str()) {
            return like_count.to_string();
        }
    }
    
    // Final fallback: search near like-related text
    search_number_near(next_data, &["like", "likes", "лайк", "лайков", "лайка"])
}

fn parse_human_number(s: &str) -> String {
    if s.is_empty() {
        return "0".to_string();
    }
    
    let trimmed = s.trim();
    let mut cleaned = String::with_capacity(trimmed.len());
    
    // More efficient character processing without multiple allocations
    for c in trimmed.chars() {
        if c != ',' && c != ' ' {
            cleaned.push(c.to_ascii_uppercase());
        }
    }
    
    // Check for multipliers more efficiently
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
    
    // Extract digits only more efficiently
    let mut result = String::new();
    for c in cleaned.chars() {
        if c.is_ascii_digit() {
            result.push(c);
        }
    }
    result
}

fn find_subscriber_count(nd: &serde_json::Value) -> String {
    // Look for subscriber count in next response
    // Path: contents.twoColumnWatchNextResults.results.results.contents[1].videoSecondaryInfoRenderer.owner.videoOwnerRenderer.subscriberCountText.simpleText
    
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
                                            if let Some(simple_text) = sub_text.get("simpleText").and_then(|t| t.as_str()) {
                                                // Extract numeric part and convert K/M abbreviations
                                                let cleaned = simple_text.replace(" подписчиков", "").replace(" подписчик", "");
                                                
                                                // Handle various formats including Russian abbreviations
                                                // Russian: тыс (thousand), млн (million)
                                                // English: K (thousand), M (million)
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
                                                // If parsing fails, return the cleaned text
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
    // First try the Python script approach
    // Look for engagement panels like in Python
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
                                                // Extract numbers from the text
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
    
    // Fallback to the original approach
    for d in [pr, nd] {
        if d.is_null() {
            continue;
        }
        // Exact match of Python logic:
        // for ct in recursive_find(d, "commentCountText") + recursive_find(d, "countText"):
        //     text = (
        //         ct.get("simpleText") or
        //         ct.get("runs", [{}])[0].get("text", "")
        //     )
        
        let comment_texts = recursive_find(d, "commentCountText");
        let count_texts = recursive_find(d, "countText");
        
        // Combine both lists like in Python (concatenation)
        let all_texts: Vec<&serde_json::Value> = comment_texts.iter().chain(count_texts.iter()).collect();
        
        for ct in all_texts {
            // Exact match of Python logic for text extraction
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
    // Fallback: search near comment-related text (like Python)
    search_number_near(nd, &["comment", "comments", "коммент", "коммента"])
}

fn translate_russian_time(time_str: &str) -> String {
    let time_lower = time_str.to_lowercase();
    
    // Russian to English translations
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
            // Also try capitalized version
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
                
                // Extract author - try multiple paths like in Python script
                let author = p
                    .get("author")
                    .and_then(|a| a.get("displayName"))
                    .and_then(|d| d.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Unknown")
                    .to_string();
                
                // Extract text content - try both direct content and runs like in Python
                let text = if let Some(content_obj) = p.get("properties").and_then(|props| props.get("content")) {
                    if let Some(content_str) = content_obj.get("content").and_then(|c| c.as_str()) {
                        content_str.to_string()
                    } else if let Some(runs) = content_obj.get("runs").and_then(|r| r.as_array()) {
                        // More efficient string building without intermediate vector
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
                    // Alternative extraction path like Python
                    let content = props.get("content").unwrap_or(&serde_json::Value::Null);
                    if let Some(runs) = content.get("runs").and_then(|r| r.as_array()) {
                        // More efficient string building without intermediate vector
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
                    // Extract published time
                    let published_at_raw = props
                        .get("publishedTime")
                        .and_then(|p| p.as_str())
                        .unwrap_or("unknown");
                    
                    // Translate Russian time expressions to English (avoid cloning if not needed)
                    let published_at = translate_russian_time(published_at_raw);
                    
                    // Extract author thumbnail
                    let author_thumbnail_raw = p
                        .get("avatar")
                        .and_then(|a| a.get("image"))
                        .and_then(|i| i.get("sources"))
                        .and_then(|s| s.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|src| src.get("url"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    
                    // Transform direct image URL to use /channel_icon/ endpoint
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
            // More efficient iteration over values
            for value in obj_map.values() {
                walk(value, comments, base_url);
            }
        } else if let Some(arr) = obj.as_array() {
            // More efficient iteration over array
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
    
    // More efficient sanitization without creating intermediate vectors
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

#[derive(Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
pub struct Comment {
    pub author: String,
    pub text: String,
    pub published_at: String,
    pub author_thumbnail: String,
    pub author_channel_url: Option<String>,
}

#[derive(Serialize, ToSchema)]
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

    // 1. Если это уже прямая ссылка на картинку — просто проксируем
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
        // Получаем channelId из страницы канала
        let handle = &input[1..];
        let page_url = format!("https://www.youtube.com/@{}", handle);

        if let Ok(resp) = client.get(&page_url).send().await {
            if let Ok(html) = resp.text().await {
                // Ищем в HTML: "channelId":"UCxxxxxxxxxxxxxxxxxxxxxx"
                if let Some(start) = html.find(r#""channelId":"UC"#) {
                    let slice = &html[start + 13..]; // после "channelId":"
                    if let Some(end) = slice.find('"') {
                        channel_id = slice[..end].to_string();
                    }
                }
                // Альтернатива: ищем link rel="canonical" href="https://www.youtube.com/channel/UC...
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
        // предполагаем video id → player endpoint
        channel_id = get_channel_id_from_video(&client, &input, &innertube_key, &ctx).await;
    }

    if channel_id.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Cannot determine channel ID"}));
    }

    // 3. Теперь получаем аватарку канала по channelId через /browse
    let avatar_url = get_channel_avatar_url(&client, &channel_id, &innertube_key, &ctx).await;

    if avatar_url.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "Channel avatar not found"}));
    }

    // 4. Проксируем найденную картинку
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

    // Use InnerTube API key from config
    let innertube_key = match config.get_innertube_key() {
        Some(key) => key,
        None => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Missing innertube_key in config.yml"
            }));
        }
    };

    let client = Client::new();
    
    // Fetch initial player response (like in youtube_fetch.py)
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
    
    // Extract ytcfg from HTML
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
    
    // Ensure region and language settings are applied
    if let Some(client) = ctx.get_mut("client").and_then(|c| c.as_object_mut()) {
        client.insert("gl".to_string(), serde_json::Value::String("US".to_string())); // Set region to USA
        client.insert("hl".to_string(), serde_json::Value::String("en-US".to_string())); // Set language to English (USA)
    }
    
    // Call InnerTube next endpoint to get additional data
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
    
    // Get comments token and fetch comments like in python script
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
    
    // Extract video details - do this once and cache results
    let vd = pr.get("videoDetails").unwrap_or(&serde_json::Value::Null);
    let micro = pr
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"))
        .unwrap_or(&serde_json::Value::Null);
    
    // Extract comments first as this may be slow, do it in parallel conceptually
    let comments = if !cont_resp.is_null() {
        extract_comments(&cont_resp, base_trimmed)
    } else {
        extract_comments(&next_data, base_trimmed)
    };
    
    // Extract likes count using improved function
    let likes = find_likes(&next_data);
    
    // Extract other metrics
    let comm_cnt = find_comments_count(&pr, &next_data);
    let subscriber_count = find_subscriber_count(&next_data);
    
    // Efficiently extract all data in one pass through the JSON
    let mut title = String::new();
    let mut author = String::new();
    let mut description = String::new();
    let mut published_at = String::new();
    let mut views = String::new();
    let mut channel_id = String::new();
    let mut channel_thumbnail = String::new();
    let duration = String::new();
    
    // Direct extraction from primary sources with minimal chaining
    if let Some(contents) = next_data.get("contents") {
        if let Some(two_col) = contents.get("twoColumnWatchNextResults") {
            if let Some(results) = two_col.get("results") {
                if let Some(results_inner) = results.get("results") {
                    if let Some(contents_array) = results_inner.get("contents").and_then(|c| c.as_array()) {
                        if contents_array.len() > 1 {
                            // Process primary info (index 0)
                            if let Some(primary_info) = contents_array[0].get("videoPrimaryInfoRenderer") {
                                // Extract title efficiently
                                if let Some(title_val) = primary_info.get("title") {
                                    title = simplify_text(title_val);
                                }
                                
                                // Extract published date efficiently
                                if let Some(date_text) = primary_info.get("dateText") {
                                    published_at = simplify_text(date_text);
                                }
                                
                                // Extract view count efficiently
                                if let Some(view_count) = primary_info.get("viewCount") {
                                    if let Some(video_view_count) = view_count.get("videoViewCountRenderer") {
                                        if let Some(view_count_simple) = video_view_count.get("viewCount") {
                                            views = simplify_text(view_count_simple);
                                            // Extract only digits from views more efficiently
                                            views.retain(|c| c.is_ascii_digit());
                                        }
                                    }
                                }
                            }
                            
                            // Process secondary info (index 1)
                            if let Some(secondary_info) = contents_array[1].get("videoSecondaryInfoRenderer") {
                                // Extract description
                                if let Some(attr_desc) = secondary_info.get("attributedDescription") {
                                    description = attr_desc.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                                }
                                
                                // Extract owner info
                                if let Some(owner) = secondary_info.get("owner").and_then(|o| o.get("videoOwnerRenderer")) {
                                    // Extract author name
                                    if let Some(title_val) = owner.get("title") {
                                        author = simplify_text(title_val);
                                    }
                                    
                                    // Extract channel ID
                                    if let Some(nav_endpoint) = owner.get("navigationEndpoint") {
                                        if let Some(browse_endpoint) = nav_endpoint.get("browseEndpoint") {
                                            channel_id = browse_endpoint.get("browseId").and_then(|b| b.as_str()).unwrap_or("").to_string();
                                        }
                                    }
                                    
                                    // Extract channel thumbnail
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
    
    // Fallback to original extraction if data not found, using cached values
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
    
    // Extract duration more efficiently
    let duration = if let Some(length_seconds) = vd.get("lengthSeconds").and_then(|l| l.as_str()) {
        if let Ok(seconds) = length_seconds.parse::<u64>() {
            format!("PT{}M{}S", seconds / 60, seconds % 60)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    
    // Build final video URL
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
                // Extract the @username part from URLs like "http://www.youtube.com/@TheAnimeSelect"
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
            // If we have a direct thumbnail URL from the API, use it
            format!("{}/channel_icon/{}", base_trimmed, urlencoding::encode(&channel_thumbnail))
        } else if !channel_id.is_empty() {
            // Otherwise use the channel ID
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
    
    // Use InnerTube API key from config
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

    // Fetch initial HTML to get ytcfg
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

    // Make first InnerTube request
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

    // Extract initial related videos
    let mut related_videos = extract_related_videos_from_response(&next_response);
    let mut continuation = get_related_continuation(&next_response);
    
    // Load more videos through continuation if needed
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

    // Remove duplicates
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

    // Apply pagination
    let start_index = offset as usize;
    let end_index = (offset + limit) as usize;
    let paginated_videos = if start_index < unique_videos.len() {
        let actual_end = std::cmp::min(end_index, unique_videos.len());
        &unique_videos[start_index..actual_end]
    } else {
        &[][..]
    };

    // Convert to our format
    let mut result_videos: Vec<RelatedVideo> = Vec::new();
    for video in paginated_videos {
        let thumbnail = format!("{}/thumbnail/{}", base_trimmed, video.video_id);
        let color = dominant_color_from_url(&format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video.video_id)).await;
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
        ("proxy" = Option<String>, Query, description = "Pass-through proxy (true/false)")
    ),
    responses(
        (status = 200, description = "Video stream"),
        (status = 400, description = "Missing video_id")
    )
)]
pub async fn direct_url(req: HttpRequest, data: web::Data<crate::AppState>) -> impl Responder {
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
    let proxy_param = query_params
        .get("proxy")
        .map(|p| p.to_lowercase())
        .unwrap_or_else(|| "true".to_string());
    let use_proxy = proxy_param != "false";

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

// Helper functions for InnerTube related videos

fn get_related_continuation(data: &serde_json::Value) -> Option<String> {
    // Try to find continuation token in the response
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
    
    // Walk through the JSON structure to find lockupViewModel entries
    walk_json_for_videos(data, &mut videos);
    
    videos
}

fn walk_json_for_videos(obj: &serde_json::Value, videos: &mut Vec<RelatedVideoInfo>) {
    match obj {
        serde_json::Value::Object(map) => {
            // Check if this is a lockupViewModel
            if let Some(lockup_view_model) = map.get("lockupViewModel") {
                if let Some(video_info) = extract_video_from_lockup(lockup_view_model) {
                    videos.push(video_info);
                }
            }
            
            // Continue walking through all values
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
    
    // Extract metadata rows
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
        // First row typically contains channel name
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
        
        // Second row typically contains views and published time
        if metadata_rows.len() > 1 {
            if let Some(second_row) = metadata_rows.get(1) {
                if let Some(metadata_parts) = second_row.as_object()
                    .and_then(|r| r.get("metadataParts"))
                    .and_then(|p| p.as_array())
                {
                    // Views (first part)
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
                    
                    // Published time (second part)
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
    
    // Extract duration from thumbnail overlays
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
    
    // Extract thumbnail
    let mut thumbnail = String::new();
    if let Some(content_image) = lockup.get("contentImage").and_then(|ci| ci.as_object()) {
        if let Some(thumbnail_vm) = content_image.get("thumbnailViewModel").and_then(|tvm| tvm.as_object()) {
            if let Some(image) = thumbnail_vm.get("image").and_then(|i| i.as_object()) {
                if let Some(sources) = image.get("sources").and_then(|s| s.as_array()) {
                    if let Some(last_source) = sources.last() {
                        if let Some(url) = last_source.as_object().and_then(|s| s.get("url")).and_then(|u| u.as_str()) {
                            thumbnail = url.to_string();
                        }
                    }
                }
            }
        }
    }
    
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

// ────────────────────────────────────────────────
// Вспомогательные функции
// ────────────────────────────────────────────────

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
                // Самый надёжный путь — c4TabbedHeaderRenderer → avatar → thumbnails
                if let Some(header) = json.pointer("/header/c4TabbedHeaderRenderer") {
                    if let Some(thumbs) = header
                        .pointer("/avatar/thumbnails")
                        .and_then(|arr| arr.as_array())
                    {
                        // Берём самую большую
                        if let Some(best) = thumbs.iter().max_by_key(|t| {
                            let w = t.pointer("/width").and_then(|w| w.as_u64()).unwrap_or(0);
                            w
                        }) {
                            if let Some(u) = best.pointer("/url").and_then(|u| u.as_str()) {
                                let mut url = u.to_string();
                                // Заменяем старый домен на новый (часто встречается)
                                if url.contains("yt3.ggpht.com") {
                                    url = url.replace("yt3.ggpht.com", "yt3.googleusercontent.com");
                                }
                                return url;
                            }
                        }
                    }
                }

                // Запасной путь — в metadata
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
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .build()
        .unwrap();

    match client.get(url).send().await {
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
