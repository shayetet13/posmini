//! Endpoints every deployment has, with or without the web pages.

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};

use crate::state::AppState;

pub async fn health() -> &'static str {
    "ok"
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
    let body = format!(
        "window.SHOP = {};\nwindow.SLUG = {};\nwindow.SHOP_INITIAL = {};\n",
        quote(&state.shop.name),
        quote(&state.shop.slug),
        quote(&initial),
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
