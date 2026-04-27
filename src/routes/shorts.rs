use actix_web::{web, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use utoipa::ToSchema;
use crate::routes::oauth::refresh_access_token;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ShortItem {
    pub video_id: String,
    pub title: String,
    pub thumbnail: String,
    pub views: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ShortsResponse {
    pub shorts: Vec<ShortItem>,
    pub sequence_token: Option<String>,
}

fn base_url(req: &HttpRequest, config: &crate::config::Config) -> String {
    if !config.server.main_url.is_empty() {
        return config.server.main_url.clone();
    }
    let info = req.connection_info();
    let scheme = info.scheme();
    let host = info.host();
    format!("{}://{}/", scheme, host.trim_end_matches('/'))
}

// Извлекает данные из ответа reel_watch_sequence или reel_item_watch
fn parse_shorts_entries(json: &serde_json::Value, base_trimmed: &str) -> (Vec<ShortItem>, Option<String>) {
    let mut shorts = Vec::new();
    let mut seen = HashSet::new();

    // 1. Пытаемся найти массив entries (характерно для reel_watch_sequence)
    if let Some(entries) = json.get("entries").and_then(|e| e.as_array()) {
        for entry in entries {
            if let Some(item) = parse_single_short(entry, base_trimmed) {
                if seen.insert(item.video_id.clone()) {
                    shorts.push(item);
                }
            }
        }
    } 
    // 2. Если entries нет, возможно это одиночный ответ reel_item_watch
    else if json.get("replacementEndpoint").is_some() {
        if let Some(item) = parse_single_short(json, base_trimmed) {
            shorts.push(item);
        }
    }

    // Ищем токен продолжения (он может быть в разных местах в зависимости от эндпоинта)
    let continuation = json.pointer("/continuationEndpoint/continuationCommand/token")
        .or_else(|| json.get("sequenceContinuation"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());

    (shorts, continuation)
}

// Парсит один объект шортса из вложенной структуры InnerTube
fn parse_single_short(entry: &serde_json::Value, base_trimmed: &str) -> Option<ShortItem> {
    // Данные могут лежать в replacementEndpoint (для reel_item_watch) или в command (для sequence)
    let endpoint = entry.get("reelWatchEndpoint")
        .or_else(|| entry.pointer("/command/reelWatchEndpoint"))
        .or_else(|| entry.pointer("/replacementEndpoint/reelWatchEndpoint"))?;

    let video_id = endpoint.get("videoId")?.as_str()?.to_string();
    
    // Пытаемся достать заголовок и просмотры из предзагруженных данных (unserializedPrefetchData)
    // Это самый надежный источник метаданных в этом API
    let prefetch_details = entry.pointer("/command/reelWatchEndpoint/unserializedPrefetchData/playerResponse/videoDetails")
        .or_else(|| entry.pointer("/unserializedPrefetchData/playerResponse/videoDetails"));

    let title = prefetch_details
        .and_then(|v| v.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("YouTube Short");

    let views = prefetch_details
        .and_then(|v| v.get("viewCount"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    Some(ShortItem {
        video_id: video_id.clone(),
        title: title.to_string(),
        thumbnail: format!("{}/thumbnail/{}", base_trimmed, video_id),
        views: views.to_string(),
    })
}

#[utoipa::path(
    get,
    path = "/get_shorts.php",
    params(
        ("sequence" = Option<String>, Query, description = "Sequence token for pagination")
    ),
    responses(
        (status = 200, description = "Shorts list", body = ShortsResponse)
    )
)]
pub async fn get_shorts(req: HttpRequest, data: web::Data<crate::AppState>, auth_config: web::Data<crate::routes::auth::AuthConfig>) -> impl Responder {
    let base = base_url(&req, &data.config);
    let base_trimmed = base.trim_end_matches('/');
    let innertube_key = data.config.get_api_key_rotated();
    
    let query_params: HashMap<String, String> = web::Query::<HashMap<String, String>>::from_query(req.query_string())
        .map(|q| q.into_inner())
        .unwrap_or_default();
    
    let sequence_params = query_params.get("sequence");
	
	let hl = query_params.get("hl").cloned().unwrap_or_else(|| "en".to_string());
    let gl = query_params.get("gl").cloned().unwrap_or_else(|| "US".to_string());

    // Выбираем эндпоинт: если токена нет — берем начальный "Seedless" ролик, если есть — последовательность
    let (url, payload) = if let Some(token) = sequence_params {
        let u = format!("https://www.youtube.com/youtubei/v1/reel/reel_watch_sequence?key={}", innertube_key);
        let p = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "WEB",
                    "clientVersion": "2.20260206.01.00",
                    "hl": hl,
                    "gl": gl
                }
            },
            "sequenceParams": token
        });
        (u, p)
    } else {
        // Начальный запрос для получения первой порции
        let u = format!("https://www.youtube.com/youtubei/v1/reel/reel_item_watch?key={}", innertube_key);
        let p = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "WEB",
                    "clientVersion": "2.20260206.01.00",
                    "hl": hl,
                    "gl": gl
                }
            },
            "inputType": "REEL_WATCH_INPUT_TYPE_SEEDLESS",
            "params": "CA8%3D",
            "disablePlayerResponse": true
        });
        (u, p)
    };

    let client = reqwest::Client::new();
	
	let refresh_token = query_params.get("token");
    let mut request_builder = client.post(&url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
		.header("X-Goog-Visitor-Id", "CgtjTS00dGRYTXhBOCif8OnOBjIoCgJQTBIiEh4SHAsMDg8QERITFBUWFxgZGhscHR4fICEiIyQlJicgSA%3D%3D");
        

    // 2. Добавляем Authorization, если есть токен
   // if let Some(t) = refresh_token {
  //      if let Ok(access_token) = crate::routes::oauth::refresh_access_token(t, &auth_config).await {
  //          request_builder = request_builder.header("Authorization", format!("Bearer {}", access_token));
  //      }
  //  }

    // 3. Выполняем запрос
    let resp = request_builder.json(&payload).send().await;

    match resp {
        Ok(raw_resp) => {
            match raw_resp.json::<serde_json::Value>().await {
                Ok(json_data) => {
                    let (shorts, next_token) = parse_shorts_entries(&json_data, base_trimmed);
                    
                    HttpResponse::Ok().json(ShortsResponse {
                        shorts,
                        sequence_token: next_token,
                    })
                },
                Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({"error": format!("JSON parse error: {}", e)}))
            }
        },
        Err(e) => HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
    }
}