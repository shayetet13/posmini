//! Endpoints every deployment has, with or without the web pages.

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};

use crate::state::AppState;

/// Home-screen label length that fits under the icon on phones.
const SHORT_NAME_CHARS: usize = 12;

pub async fn health() -> &'static str {
    "ok"
}

/// The web app manifest: what the installed app is called and which screens it offers. Built from
/// the shop settings, so the home-screen name is the shop's own.
pub async fn manifest(State(state): State<AppState>) -> Response {
    let name = state.shop.name.trim();
    let short: String = name.chars().take(SHORT_NAME_CHARS).collect();
    let body = serde_json::json!({
        "id": "/pos",
        "name": format!("{name} POS"),
        "short_name": short,
        "description": "ระบบขายหน้าร้าน รับเงิน เรียกคิว และพิมพ์บิล",
        "lang": "th",
        "dir": "ltr",
        "start_url": "/pos",
        "scope": "/",
        "display": "standalone",
        "orientation": "any",
        "background_color": "#F6F0E6",
        "theme_color": "#F6F0E6",
        "categories": ["business", "shopping"],
        "icons": [
            { "src": "/static/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
            { "src": "/static/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any" },
            { "src": "/static/icon-maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" },
        ],
        "shortcuts": [
            { "name": "ขายหน้าร้าน", "url": "/pos", "icons": [{ "src": "/static/icon-192.png", "sizes": "192x192" }] },
            { "name": "จอลูกค้า", "url": format!("/display/{}", state.shop.slug), "icons": [{ "src": "/static/icon-192.png", "sizes": "192x192" }] },
            { "name": "สรุปยอดขาย", "url": "/dashboard", "icons": [{ "src": "/static/icon-192.png", "sizes": "192x192" }] },
        ],
    });
    (
        [
            (
                header::CONTENT_TYPE,
                "application/manifest+json; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body.to_string(),
    )
        .into_response()
}

/// The shop's name for the static pages: `window.SHOP`, `window.SLUG`, `window.SHOP_INITIAL`.
pub async fn shop_js(State(state): State<AppState>) -> Response {
    let initial = state
        .shop
        .name
        .chars()
        .next()
        .map(String::from)
        .unwrap_or_default();
    let quote = |text: &str| serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    let bill = state.bill();
    let body = format!(
        "window.SHOP = {};\nwindow.SLUG = {};\nwindow.SHOP_INITIAL = {};\nwindow.DISPLAY_NAME = {};\n",
        quote(&state.shop.name),
        quote(&state.shop.slug),
        quote(&initial),
        quote(bill.display_title(&state.shop.name)),
    );
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}
