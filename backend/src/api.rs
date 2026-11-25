use std::{convert::Infallible, net::SocketAddr, time::Duration};

use axum::{
    Json,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, header},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, Sse},
    },
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    auth::{Authed, client_key},
    error::{ApiError, ApiResult},
    events::{self, Event},
    orders::{self, ItemReq, OrderView},
    promptpay,
    state::AppState,
};

const SELF_ORDER_MAX: usize = 6;
const SELF_ORDER_WINDOW: Duration = Duration::from_secs(60);
const MAX_CART_LINES: usize = 60;
const MAX_NAME_CHARS: usize = 80;
const REFRESH_MAX: usize = 8;
const REFRESH_WINDOW: Duration = Duration::from_secs(600);
const DISPLAY_KINDS: [&str; 6] = ["cart", "pay", "paid", "called", "idle", "refresh"];
const CUSTOMER_KINDS: [&str; 4] = ["order", "pay", "paid", "called"];

#[derive(Serialize, sqlx::FromRow)]
pub struct ProductView {
    id: i64,
    name: String,
    category: String,
    price_satang: i64,
    /// The group this row sits in; `None` = top level.
    parent_id: Option<i64>,
    /// A group holds other rows and cannot be ordered itself.
    is_group: bool,
    /// Card colour `#RRGGBB`.
    color: Option<String>,
    /// Cache-busting version of the product photo; `None` when there is no photo.
    image_v: Option<i64>,
}

async fn list_products(state: &AppState) -> ApiResult<Vec<ProductView>> {
    Ok(sqlx::query_as(
        "SELECT id, name, category, price_satang, parent_id, is_group, color, \
         CASE WHEN image_mime IS NULL THEN NULL ELSE image_v END AS image_v \
         FROM products WHERE shop_id = ? AND active = 1 ORDER BY sort, id",
    )
    .bind(state.shop.id)
    .fetch_all(&state.pool)
    .await?)
}

fn check_slug(state: &AppState, slug: &str) -> ApiResult<()> {
    if slug == state.shop.slug {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

fn check_token(token: &str) -> ApiResult<()> {
    let ok = token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit());
    if ok { Ok(()) } else { Err(ApiError::NotFound) }
}

pub async fn products(State(state): State<AppState>) -> ApiResult<Json<Vec<ProductView>>> {
    Ok(Json(list_products(&state).await?))
}

pub async fn public_products(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<Vec<ProductView>>> {
    check_slug(&state, &slug)?;
    Ok(Json(list_products(&state).await?))
}

#[derive(Deserialize)]
pub struct PosOrderReq {
    method: String,
    items: Vec<ItemReq>,
    /// Cash handed over (satang); the bill shows it and the change.
    #[serde(default)]
    cash_received_satang: Option<i64>,
}

pub async fn pos_create_order(
    _auth: Authed,
    State(state): State<AppState>,
    Json(req): Json<PosOrderReq>,
) -> ApiResult<Json<OrderView>> {
    if !matches!(req.method.as_str(), "cash" | "promptpay") {
        return Err(ApiError::BadRequest("วิธีชำระเงินไม่ถูกต้อง"));
    }
    let id = orders::create_order(
        &state,
        "pos",
        &req.method,
        &req.items,
        req.cash_received_satang,
    )
    .await?;
    if req.method == "cash" {
        orders::mark_paid(&state, id, None).await?;
    } else {
        orders::start_payment(&state, id).await?;
    }
    state.publish(Event::new("orders", json!({})));
    let view = orders::active_orders(&state)
        .await?
        .into_iter()
        .find(|o| o.id == id);
    view.map(Json).ok_or(ApiError::NotFound)
}

#[derive(Deserialize)]
pub struct SelfOrderReq {
    items: Vec<ItemReq>,
}

pub async fn self_order(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<SelfOrderReq>,
) -> ApiResult<Json<Value>> {
    check_slug(&state, &slug)?;
    let key = format!("self:{}", client_key(&headers, peer, state.cfg.trust_proxy));
    if state.is_limited(&key, SELF_ORDER_MAX, SELF_ORDER_WINDOW) {
        return Err(ApiError::TooMany);
    }
    state.record_hit(&key);
    // Refuse before anything is created when a payment could not be verified automatically.
    orders::ensure_verifiable(&state).await?;
    // From the "is another QR open?" check until this customer's QR is registered, one at a time.
    let _one_qr = state.qr_lock().lock().await;
    orders::ensure_qr_free(&state, None).await?;
    let id = orders::create_order(&state, "self", "promptpay", &req.items, None).await?;
    if let Err(err) = orders::start_verified_payment(&state, id).await {
        // The customer is told to try again; an order with no payable QR must not linger.
        if orders::close_order(&state, id, false).await.is_err() {
            tracing::warn!(
                order_id = id,
                "could not cancel an order that has no payable QR"
            );
        }
        return Err(err);
    }
    let token: String = sqlx::query_scalar("SELECT token FROM orders WHERE id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(json!({ "token": token })))
}

pub async fn order_by_token(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> ApiResult<Json<Value>> {
    check_token(&token)?;
    let order = orders::view_by_token(&state, &token).await?;
    let bill = state.bill();
    let mut body = serde_json::to_value(order).map_err(|e| anyhow::anyhow!(e))?;
    // What the on-screen receipt shows around the items: same wording as the printed one.
    body["bill"] = json!({
        "shop": bill.shop_title(&state.shop.name),
        "header": bill.header_lines(),
        "footer": bill.footer.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>(),
        "title": bill.ticket_title.trim(),
        "show_time": bill.show_time,
        "show_method": bill.show_method,
    });
    Ok(Json(body))
}

/// A customer who ordered from their phone asks for a fresh QR once theirs has expired.
pub async fn refresh_qr(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> ApiResult<Json<Value>> {
    check_token(&token)?;
    let key = format!("refresh:{token}");
    if state.is_limited(&key, REFRESH_MAX, REFRESH_WINDOW) {
        return Err(ApiError::TooMany);
    }
    state.record_hit(&key);
    let order = orders::view_by_token(&state, &token).await?;
    let expired = matches!(order.pay_state.as_deref(), Some("expired" | "cancelled"))
        || (order.pay_state.as_deref() == Some("pending") && order.qr_expires_in == Some(0));
    if order.status != "pending_payment" || order.source != "self" || !expired {
        return Err(ApiError::BadRequest("ยังไม่ต้องสร้าง QR ใหม่"));
    }
    orders::ensure_verifiable(&state).await?;
    let _one_qr = state.qr_lock().lock().await;
    orders::ensure_qr_free(&state, Some(order.id)).await?;
    orders::start_verified_payment(&state, order.id).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn active_orders(
    _auth: Authed,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<OrderView>>> {
    Ok(Json(orders::active_orders(&state).await?))
}

pub async fn confirm_order(
    _auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let changed = orders::mark_paid(&state, id, None).await?;
    Ok(Json(json!({ "changed": changed })))
}

#[derive(Deserialize)]
pub struct CallParams {
    #[serde(default)]
    force: bool,
}

pub async fn call_order(
    _auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<CallParams>,
) -> ApiResult<Json<Value>> {
    orders::call_order(&state, id, params.force).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Calls the queue that has waited longest.
pub async fn call_next(_auth: Authed, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let id = orders::call_next(&state).await?;
    Ok(Json(json!({ "ok": true, "order_id": id })))
}

pub async fn done_order(
    _auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    orders::close_order(&state, id, true).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn cancel_order(
    _auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    orders::close_order(&state, id, false).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn repay_order(
    _auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM orders WHERE id = ? AND shop_id = ?")
            .bind(id)
            .bind(state.shop.id)
            .fetch_optional(&state.pool)
            .await?;
    if status.as_deref() != Some("pending_payment") {
        return Err(ApiError::BadRequest("ออเดอร์นี้ไม่ได้รอชำระเงิน"));
    }
    orders::start_payment(&state, id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Serialize)]
pub struct CartLine {
    name: String,
    qty: i64,
    price_satang: i64,
}

#[derive(Deserialize)]
pub struct CartReq {
    items: Vec<CartLine>,
}

pub async fn cart(
    _auth: Authed,
    State(state): State<AppState>,
    Json(req): Json<CartReq>,
) -> ApiResult<Json<Value>> {
    let valid = req.items.len() <= MAX_CART_LINES
        && req.items.iter().all(|l| {
            l.name.chars().count() <= MAX_NAME_CHARS
                && (1..=99).contains(&l.qty)
                && l.price_satang >= 0
        });
    if !valid {
        return Err(ApiError::BadRequest("ข้อมูลตะกร้าไม่ถูกต้อง"));
    }
    let total: i64 = req.items.iter().map(|l| l.qty * l.price_satang).sum();
    let cart = json!({ "items": req.items, "total_satang": total });
    state.set_cart(cart.clone());
    state.publish(Event::new("cart", cart));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct QrQuery {
    text: String,
}

pub async fn qr_image(_auth: Authed, Query(query): Query<QrQuery>) -> ApiResult<Response> {
    if query.text.is_empty() || query.text.len() > 400 {
        return Err(ApiError::BadRequest("ข้อความสำหรับ QR ไม่ถูกต้อง"));
    }
    let svg = promptpay::qr_svg(&query.text).ok_or(ApiError::BadRequest("สร้าง QR ไม่ได้"))?;
    Ok(([(header::CONTENT_TYPE, "image/svg+xml")], svg).into_response())
}

pub async fn pos_events(
    _auth: Authed,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    events::stream(state.hub.clone(), |_| true)
}

pub async fn display_events(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    check_slug(&state, &slug)?;
    let connected = state.display_connected();
    Ok(events::stream(state.hub.clone(), move |ev| {
        let _still_counted = &connected;
        // A customer paying on their own phone must not take over the counter display.
        DISPLAY_KINDS.contains(&ev.kind) && ev.data["source"] != "self"
    }))
}

/// Where the customer display stands right now, so one opened late (or reconnecting) catches up.
pub async fn display_state(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<Value>> {
    check_slug(&state, &slug)?;
    Ok(Json(
        json!({ "pay": orders::display_pay(&state).await?, "cart": state.cart() }),
    ))
}

pub async fn displays(_auth: Authed, State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "count": state.displays() }))
}

pub async fn customer_events(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    check_token(&token)?;
    Ok(events::stream(state.hub.clone(), move |ev| {
        CUSTOMER_KINDS.contains(&ev.kind) && ev.token.as_deref() == Some(token.as_str())
    }))
}
