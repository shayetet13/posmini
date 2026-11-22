//! The web pages, for the all-in-one mode (`SERVE_UI=true`, the default). They are plain static
//! HTML from `frontend/public`; the shop name and the menu rail are filled in by the browser
//! (`/shop.js` + `common.js`), so the very same files can be served by a separate frontend
//! service in the cloud.

use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};

const LOGIN: &str = include_str!("../../frontend/public/login.html");
const POS: &str = include_str!("../../frontend/public/pos.html");
const SETTINGS: &str = include_str!("../../frontend/public/settings.html");
const DASHBOARD: &str = include_str!("../../frontend/public/dashboard.html");
const DISPLAY: &str = include_str!("../../frontend/public/display.html");
const MENU: &str = include_str!("../../frontend/public/menu.html");
const QUEUE: &str = include_str!("../../frontend/public/queue.html");

pub async fn root() -> Redirect {
    Redirect::to("/pos")
}

pub async fn login_page() -> Html<&'static str> {
    Html(LOGIN)
}

pub async fn pos_page() -> Html<&'static str> {
    Html(POS)
}

pub async fn settings_page() -> Html<&'static str> {
    Html(SETTINGS)
}

pub async fn dashboard_page() -> Html<&'static str> {
    Html(DASHBOARD)
}

pub async fn display_page(Path(_slug): Path<String>) -> Html<&'static str> {
    Html(DISPLAY)
}

pub async fn menu_page(Path(_slug): Path<String>) -> Html<&'static str> {
    Html(MENU)
}

pub async fn queue_page(Path(_token): Path<String>) -> Html<&'static str> {
    Html(QUEUE)
}

pub async fn asset(Path(name): Path<String>) -> Response {
    const JS: &str = "text/javascript; charset=utf-8";
    const PNG: &str = "image/png";
    // Logos: replace the files in static/brand with the official high-resolution artwork.
    let (body, mime): (&'static [u8], &'static str) = match name.as_str() {
        "app.css" => (
            include_bytes!("../../frontend/public/static/app.css"),
            "text/css; charset=utf-8",
        ),
        "common.js" => (include_bytes!("../../frontend/public/static/common.js"), JS),
        "station.js" => (
            include_bytes!("../../frontend/public/static/station.js"),
            JS,
        ),
        "calc.js" => (include_bytes!("../../frontend/public/static/calc.js"), JS),
        "pos.js" => (include_bytes!("../../frontend/public/static/pos.js"), JS),
        "dashboard.js" => (
            include_bytes!("../../frontend/public/static/dashboard.js"),
            JS,
        ),
        "settings.js" => (
            include_bytes!("../../frontend/public/static/settings.js"),
            JS,
        ),
        "display.js" => (
            include_bytes!("../../frontend/public/static/display.js"),
            JS,
        ),
        "menu.js" => (include_bytes!("../../frontend/public/static/menu.js"), JS),
        "queue.js" => (include_bytes!("../../frontend/public/static/queue.js"), JS),
        "brand-thaiqr-logo.png" => (
            include_bytes!("../../frontend/public/static/brand-thaiqr-logo.png"),
            PNG,
        ),
        "brand-promptpay-logo.png" => (
            include_bytes!("../../frontend/public/static/brand-promptpay-logo.png"),
            PNG,
        ),
        "brand-thaiqr-mark.png" => (
            include_bytes!("../../frontend/public/static/brand-thaiqr-mark.png"),
            PNG,
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let cache = if mime == PNG {
        "public, max-age=86400"
    } else {
        "no-cache"
    };
    (
        [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)],
        body,
    )
        .into_response()
}
