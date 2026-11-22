mod admin;
mod api;
mod auth;
mod bill;
mod config;
mod dashboard;
mod db;
mod error;
mod escpos;
mod events;
#[cfg(test)]
mod http_tests;
mod orders;
mod pages;
mod payment;
mod print_api;
mod printing;
mod printq;
mod promptpay;
mod raster;
mod settings;
mod site;
mod state;
mod thai;

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post, put},
};
use tokio::net::TcpListener;

use crate::{
    config::Config,
    state::{AppState, PayRuntime},
};

/// The web pages themselves; left out when a separate frontend service serves them.
fn pages_router() -> Router<AppState> {
    Router::new()
        .route("/", get(pages::root))
        .route("/login", get(pages::login_page))
        .route("/pos", get(pages::pos_page))
        .route("/settings", get(pages::settings_page))
        .route("/dashboard", get(pages::dashboard_page))
        .route("/display/{slug}", get(pages::display_page))
        .route("/m/{slug}", get(pages::menu_page))
        .route("/q/{token}", get(pages::queue_page))
        .route("/static/{name}", get(pages::asset))
}

fn router(state: AppState) -> Router {
    let ui = state.cfg.serve_ui;
    let app = Router::new()
        .route("/health", get(site::health))
        .route("/shop.js", get(site::shop_js))
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/api/dashboard", get(dashboard::data))
        .route("/api/dashboard/export", get(dashboard::export))
        .route("/img/{id}", get(admin::image))
        .route(
            "/api/products",
            get(api::products)
                .post(admin::create_product)
                .delete(admin::delete_all_products),
        )
        .route(
            "/api/products/bulk-delete",
            post(admin::bulk_delete_products),
        )
        .route(
            "/api/products/{id}",
            put(admin::update_product).delete(admin::delete_product),
        )
        .route(
            "/api/products/{id}/image",
            put(admin::set_image)
                .delete(admin::clear_image)
                .layer(DefaultBodyLimit::max(admin::IMAGE_MAX_BYTES)),
        )
        .route(
            "/api/settings",
            get(admin::get_settings).put(admin::put_settings),
        )
        .route("/api/settings/test", post(admin::test_settings))
        .route(
            "/api/print/settings",
            get(print_api::get_settings).put(print_api::put_settings),
        )
        .route(
            "/api/bill/settings",
            get(print_api::get_bill).put(print_api::put_bill),
        )
        .route("/api/bill/preview", post(print_api::preview))
        .route("/api/print/queue", get(print_api::queue_status))
        .route("/api/print/station/claim", post(print_api::station_claim))
        .route("/api/print/station/done", post(print_api::station_done))
        .route("/api/print/station/fail", post(print_api::station_fail))
        .route("/api/print/queue/skip", post(print_api::queue_skip))
        .route("/api/print/queue/retry", post(print_api::queue_retry))
        .route("/api/print/targets", get(print_api::targets))
        .route("/api/print/test", post(print_api::test))
        .route("/api/orders/{id}/print", post(print_api::print_order))
        .route("/api/m/{slug}/products", get(api::public_products))
        .route("/api/m/{slug}/orders", post(api::self_order))
        .route("/api/q/{token}", get(api::order_by_token))
        .route("/api/q/{token}/refresh", post(api::refresh_qr))
        .route("/api/orders", post(api::pos_create_order))
        .route("/api/orders/active", get(api::active_orders))
        .route("/api/orders/{id}/confirm", post(api::confirm_order))
        .route("/api/orders/call-next", post(api::call_next))
        .route("/api/orders/{id}/call", post(api::call_order))
        .route("/api/orders/{id}/done", post(api::done_order))
        .route("/api/orders/{id}/cancel", post(api::cancel_order))
        .route("/api/orders/{id}/repay", post(api::repay_order))
        .route("/api/cart", post(api::cart))
        .route("/api/qr", get(api::qr_image))
        .route("/events/pos", get(api::pos_events))
        .route("/events/display/{slug}", get(api::display_events))
        .route("/api/display/{slug}/state", get(api::display_state))
        .route("/api/displays", get(api::displays))
        .route("/events/q/{token}", get(api::customer_events));
    let app = if ui { app.merge(pages_router()) } else { app };
    app.with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().with_target(false).init();

    let cfg = Config::from_env()?;
    let pool = db::connect(&cfg.database_url).await?;
    let shop = db::seed(&pool, &cfg).await?;
    let pay = PayRuntime::build(settings::load(&pool, shop.id, &cfg.pay_defaults).await?)?;
    let print = printing::load(&pool, shop.id).await?;
    let bill = bill::load(&pool, shop.id).await?;
    let bind_addr = cfg.bind_addr.clone();
    let state = AppState::new(pool, cfg, shop, pay, print, bill);

    tokio::spawn(orders::run_poller(state.clone()));
    tokio::spawn(printq::run_worker(state.clone()));

    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("bind {bind_addr}"))?;
    tracing::info!("posmini listening on http://{bind_addr}");
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("server error")
}
