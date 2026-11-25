use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{AssertSqlSafe, SqlitePool};

use crate::{
    error::{ApiError, ApiResult},
    events::Event,
    payment::{BotState, PaymentClient, RemotePayment, format_baht},
    printing, printq, promptpay,
    settings::PaySettings,
    state::{AppState, random_token},
    thai,
};

const MAX_LINES: usize = 40;
const MAX_QTY: i64 = 99;
/// Largest cash amount accepted from the keypad (฿100,000).
const MAX_CASH_SATANG: i64 = 10_000_000;
/// A PromptPay QR is valid for 2 minutes (the verify API's default window).
const QR_TTL_SECS: u32 = 120;
/// The verify API never allows a longer window, so a larger value is bad data.
const MAX_QR_SECS: i64 = 600;
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const BANGKOK_OFFSET_HOURS: i64 = 7;

const ITEM_NOTE_MAX_CHARS: usize = 60;
const ORDER_NOTE_MAX_CHARS: usize = 100;
const REASON_MAX_CHARS: usize = 60;
/// An order never goes below one baht: the QR and the till need a real amount.
const MIN_TOTAL_SATANG: i64 = 100;

#[derive(Deserialize)]
pub struct ItemReq {
    pub product_id: i64,
    pub qty: i64,
    /// What the kitchen should know about this line ("no sugar").
    #[serde(default)]
    pub note: String,
}

/// A discount on the whole bill: `percent` takes `value` whole percent off, `amount` takes
/// `value` satang off. The server works the figure out, the till only asks.
#[derive(Deserialize)]
pub struct DiscountReq {
    pub kind: String,
    pub value: i64,
    #[serde(default)]
    pub reason: String,
}

/// What a till can add to an order besides its lines.
#[derive(Deserialize, Default)]
pub struct OrderExtras {
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub discount: Option<DiscountReq>,
    /// Made by a till that sold with no connection; sending the same one again changes nothing.
    #[serde(default)]
    pub client_id: Option<String>,
    /// When the till made that sale (its own clock), kept for the record only.
    #[serde(default)]
    pub offline_at: Option<String>,
}

const CLIENT_ID_MIN: usize = 8;
const CLIENT_ID_MAX: usize = 64;
const OFFLINE_AT_MAX_CHARS: usize = 40;

fn clean_client_id(value: Option<&str>) -> ApiResult<Option<String>> {
    let Some(id) = value else { return Ok(None) };
    let ok = (CLIENT_ID_MIN..=CLIENT_ID_MAX).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then(|| Some(id.to_string()))
        .ok_or(ApiError::BadRequest("รหัสรายการออฟไลน์ไม่ถูกต้อง"))
}

/// The order a till already sent under this id, if any.
pub async fn id_by_client_id(state: &AppState, client_id: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT id FROM orders WHERE shop_id = ? AND client_id = ?")
        .bind(state.shop.id)
        .bind(client_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ItemView {
    pub name: String,
    pub qty: i64,
    pub unit_price_satang: i64,
    pub note: String,
}

/// The satang a discount takes off `subtotal`, or why it is refused.
fn discount_satang(subtotal: i64, discount: &DiscountReq) -> ApiResult<i64> {
    let amount = match discount.kind.as_str() {
        "percent" if (1..=99).contains(&discount.value) => subtotal * discount.value / 100,
        "percent" => return Err(ApiError::BadRequest("ส่วนลดเปอร์เซ็นต์ต้องอยู่ระหว่าง 1-99")),
        "amount" if discount.value >= 1 => discount.value,
        _ => return Err(ApiError::BadRequest("ส่วนลดไม่ถูกต้อง")),
    };
    if amount < 1 {
        return Err(ApiError::BadRequest("ส่วนลดน้อยเกินไป"));
    }
    if subtotal - amount < MIN_TOTAL_SATANG {
        return Err(ApiError::BadRequest("ส่วนลดมากเกินไป ยอดสุทธิต้องไม่ต่ำกว่า ฿1"));
    }
    Ok(amount)
}

fn clean_note(value: &str, max_chars: usize, error: &'static str) -> ApiResult<String> {
    crate::admin::clean_text(value, max_chars).ok_or(ApiError::BadRequest(error))
}

#[derive(sqlx::FromRow)]
struct OrderRow {
    id: i64,
    token: String,
    queue_no: Option<i64>,
    status: String,
    method: String,
    source: String,
    total_satang: i64,
    pay_state: Option<String>,
    qr_payload: Option<String>,
    pay_note: Option<String>,
    pay_expires_at: Option<String>,
    cash_received_satang: Option<i64>,
    created_at: String,
    note: String,
    discount_satang: i64,
    discount_reason: String,
    cancel_reason: String,
}

#[derive(Serialize)]
pub struct OrderView {
    pub id: i64,
    pub token: String,
    pub queue_no: Option<i64>,
    pub queue_label: Option<String>,
    pub status: String,
    pub method: String,
    pub source: String,
    /// What the customer pays, after the discount.
    pub total_satang: i64,
    /// The lines added up, before the discount.
    pub subtotal_satang: i64,
    pub discount_satang: i64,
    pub discount_reason: String,
    pub note: String,
    pub cancel_reason: String,
    /// Cash the customer handed over (cash orders paid at the counter).
    pub cash_received_satang: Option<i64>,
    /// Cash handed over minus the total.
    pub change_satang: Option<i64>,
    pub pay_state: Option<String>,
    /// Why automatic payment checking was not used for this order (shown to the cashier).
    pub pay_note: Option<String>,
    pub qr_svg: Option<String>,
    /// Seconds the QR stays valid, counted when this view was built (0 = already expired).
    /// Only for QRs that really expire; `None` for the static fallback QR.
    pub qr_expires_in: Option<i64>,
    pub ahead: Option<i64>,
    pub created_at: String,
    pub items: Vec<ItemView>,
}

const ORDER_COLUMNS: &str = "id, token, queue_no, status, method, source, total_satang, \
                             pay_state, qr_payload, pay_note, pay_expires_at, cash_received_satang, \
                             created_at, note, discount_satang, discount_reason, cancel_reason";

fn today() -> String {
    (chrono::Utc::now() + chrono::Duration::hours(BANGKOK_OFFSET_HOURS))
        .format("%Y-%m-%d")
        .to_string()
}

/// Whole seconds left until `expires_at` (database UTC text), never negative.
fn remaining_secs(expires_at: Option<&str>, now: chrono::NaiveDateTime) -> Option<i64> {
    let expires = chrono::NaiveDateTime::parse_from_str(expires_at?, "%Y-%m-%d %H:%M:%S").ok()?;
    Some((expires - now).num_seconds().clamp(0, MAX_QR_SECS))
}

fn in_secs(secs: u32) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(i64::from(secs)))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn shows_qr(pay_state: Option<&str>) -> bool {
    matches!(pay_state, Some("pending" | "manual"))
}

async fn build_view(state: &AppState, row: OrderRow) -> ApiResult<OrderView> {
    let items: Vec<ItemView> = sqlx::query_as(
        "SELECT name, qty, unit_price_satang, note FROM order_items WHERE order_id = ? ORDER BY id",
    )
    .bind(row.id)
    .fetch_all(&state.pool)
    .await?;
    let ahead = match (row.status.as_str(), row.queue_no) {
        ("paid", Some(no)) => Some(
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM orders WHERE shop_id = ? AND queue_date = ? \
                 AND status = 'paid' AND queue_no < ?",
            )
            .bind(state.shop.id)
            .bind(today())
            .bind(no)
            .fetch_one(&state.pool)
            .await?,
        ),
        _ => None,
    };
    let qr_svg = if shows_qr(row.pay_state.as_deref()) && row.status == "pending_payment" {
        row.qr_payload.as_deref().and_then(promptpay::qr_svg)
    } else {
        None
    };
    let qr_expires_in = match (&qr_svg, row.pay_state.as_deref()) {
        (Some(_), Some("pending")) => remaining_secs(
            row.pay_expires_at.as_deref(),
            chrono::Utc::now().naive_utc(),
        ),
        _ => None,
    };
    let change_satang = row
        .cash_received_satang
        .filter(|_| row.method == "cash")
        .map(|received| received - row.total_satang);
    Ok(OrderView {
        id: row.id,
        token: row.token,
        queue_no: row.queue_no,
        queue_label: row.queue_no.map(|no| state.bill().queue_label(no)),
        status: row.status,
        method: row.method,
        source: row.source,
        total_satang: row.total_satang,
        subtotal_satang: row.total_satang + row.discount_satang,
        discount_satang: row.discount_satang,
        discount_reason: row.discount_reason,
        note: row.note,
        cancel_reason: row.cancel_reason,
        cash_received_satang: row.cash_received_satang,
        change_satang,
        pay_state: row.pay_state,
        pay_note: row.pay_note,
        qr_svg,
        qr_expires_in,
        ahead,
        created_at: row.created_at,
        items,
    })
}

pub async fn view_by_token(state: &AppState, token: &str) -> ApiResult<OrderView> {
    let row: Option<OrderRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {ORDER_COLUMNS} FROM orders WHERE shop_id = ? AND token = ?"
    )))
    .bind(state.shop.id)
    .bind(token)
    .fetch_optional(&state.pool)
    .await?;
    let view = build_view(state, row.ok_or(ApiError::NotFound)?).await?;
    // Public endpoint: internal payment-API errors are for the cashier only.
    Ok(OrderView {
        pay_note: None,
        ..view
    })
}

pub async fn view_by_id(state: &AppState, id: i64) -> ApiResult<OrderView> {
    let row: Option<OrderRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {ORDER_COLUMNS} FROM orders WHERE shop_id = ? AND id = ?"
    )))
    .bind(state.shop.id)
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    build_view(state, row.ok_or(ApiError::NotFound)?).await
}

pub async fn active_orders(state: &AppState) -> ApiResult<Vec<OrderView>> {
    let rows: Vec<OrderRow> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {ORDER_COLUMNS} FROM orders WHERE shop_id = ? AND queue_date = ? \
         AND status IN ('pending_payment', 'paid', 'called') ORDER BY queue_no IS NULL, queue_no, id"
    )))
    .bind(state.shop.id)
    .bind(today())
    .fetch_all(&state.pool)
    .await?;
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        views.push(build_view(state, row).await?);
    }
    Ok(views)
}

pub async fn create_order(
    state: &AppState,
    source: &str,
    method: &str,
    items: &[ItemReq],
    extras: &OrderExtras,
    cash_received: Option<i64>,
) -> ApiResult<i64> {
    if items.is_empty() || items.len() > MAX_LINES {
        return Err(ApiError::BadRequest("จำนวนรายการไม่ถูกต้อง"));
    }
    let client_id = clean_client_id(extras.client_id.as_deref())?;
    let offline_at = match &extras.offline_at {
        Some(at) => Some(clean_note(
            at,
            OFFLINE_AT_MAX_CHARS,
            "เวลาขายออฟไลน์ไม่ถูกต้อง",
        )?),
        None => None,
    };
    let order_note = clean_note(
        &extras.note,
        ORDER_NOTE_MAX_CHARS,
        "โน้ตของออเดอร์ยาวเกิน 100 ตัวอักษร",
    )?;
    let mut lines: Vec<(i64, String, i64, i64, String)> = Vec::with_capacity(items.len());
    for item in items {
        if !(1..=MAX_QTY).contains(&item.qty) {
            return Err(ApiError::BadRequest("จำนวนสินค้าไม่ถูกต้อง"));
        }
        let note = clean_note(
            &item.note,
            ITEM_NOTE_MAX_CHARS,
            "โน้ตของรายการยาวเกิน 60 ตัวอักษร",
        )?;
        let product: Option<(String, i64, i64)> = sqlx::query_as(
            "SELECT name, price_satang, sold_out FROM products \
             WHERE id = ? AND shop_id = ? AND active = 1 AND is_group = 0",
        )
        .bind(item.product_id)
        .bind(state.shop.id)
        .fetch_optional(&state.pool)
        .await?;
        let (name, price, sold_out) = product.ok_or(ApiError::BadRequest("ไม่พบสินค้า"))?;
        if sold_out == 1 {
            return Err(ApiError::Conflict(format!("“{name}” หมดแล้ว")));
        }
        lines.push((item.product_id, name, price, item.qty, note));
    }
    let subtotal: i64 = lines.iter().map(|(_, _, price, qty, _)| price * qty).sum();
    let (discount, discount_reason) = match &extras.discount {
        Some(d) => (
            discount_satang(subtotal, d)?,
            clean_note(&d.reason, REASON_MAX_CHARS, "เหตุผลส่วนลดยาวเกิน 60 ตัวอักษร")?,
        ),
        None => (0, String::new()),
    };
    let total = subtotal - discount;
    let cash_received = cash_received.filter(|_| method == "cash");
    if let Some(received) = cash_received
        && !(total..=MAX_CASH_SATANG).contains(&received)
    {
        return Err(ApiError::BadRequest("รับเงินมาไม่พอ หรือมากเกินไป"));
    }

    let mut tx = state.pool.begin().await?;
    let order_id: i64 = sqlx::query_scalar(
        "INSERT INTO orders (shop_id, token, queue_date, status, method, source, total_satang, \
         cash_received_satang, note, discount_satang, discount_reason, client_id, offline_at) \
         VALUES (?, ?, ?, 'pending_payment', ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(state.shop.id)
    .bind(random_token())
    .bind(today())
    .bind(method)
    .bind(source)
    .bind(total)
    .bind(cash_received)
    .bind(&order_note)
    .bind(discount)
    .bind(&discount_reason)
    .bind(&client_id)
    .bind(&offline_at)
    .fetch_one(&mut *tx)
    .await?;
    for (product_id, name, price, qty, note) in &lines {
        sqlx::query(
            "INSERT INTO order_items (order_id, product_id, name, unit_price_satang, qty, note) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(order_id)
        .bind(product_id)
        .bind(name)
        .bind(price)
        .bind(qty)
        .bind(note)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(order_id)
}

/// The verify-API payment of this order that is still waiting (queued or pending), if any.
async fn waiting_remote(state: &AppState, order_id: i64) -> Option<String> {
    sqlx::query_scalar(
        "SELECT pay_ref FROM orders WHERE id = ? AND shop_id = ? AND pay_ref IS NOT NULL \
         AND pay_state IN ('queued', 'pending')",
    )
    .bind(order_id)
    .bind(state.shop.id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

/// Frees the order's place in the verify API's queue. Payments with the same amount and
/// destination wait in line, so an abandoned one would hold up every later one until it expires.
async fn cancel_remote(state: &AppState, pay_ref: &str) {
    let pay = state.pay();
    let Some(client) = &pay.client else {
        return;
    };
    if let Err(err) = client.cancel(pay_ref).await {
        tracing::info!(error = %format!("{err:#}"), pay_ref, "remote payment not cancelled (probably already finished)");
    }
}

fn cancel_remote_in_background(state: &AppState, pay_ref: Option<String>) {
    if let Some(pay_ref) = pay_ref {
        let state = state.clone();
        tokio::spawn(async move { cancel_remote(&state, &pay_ref).await });
    }
}

/// Customers who order from their phone only get a QR the verify API is watching: a plain
/// PromptPay QR would be paid but never noticed, so the order would stay unpaid.
const PAY_UNAVAILABLE: &str = "ระบบชำระเงินอัตโนมัติยังไม่พร้อม กรุณาลองใหม่อีกครั้ง หรือแจ้งพนักงานที่เคาน์เตอร์";

/// What `pay_ref`, `pay_state`, `qr_payload`, `pay_note` and `pay_expires_at` become.
struct Started {
    pay_ref: Option<String>,
    pay_state: String,
    qr_payload: Option<String>,
    note: Option<String>,
    expires_at: Option<String>,
}

/// Registers the order's payment with the verify API.
async fn open_remote(
    client: &PaymentClient,
    settings: &PaySettings,
    total_satang: i64,
) -> anyhow::Result<Started> {
    let payment = client
        .create(total_satang, &random_token(), QR_TTL_SECS)
        .await?;
    let pending = payment.status == "pending";
    let qr_payload = payment
        .qr_payload
        .or_else(|| pending.then(|| static_qr(settings, total_satang)).flatten());
    // The clock starts when the QR is usable; a queued payment has no deadline yet.
    let expires_at = pending.then(|| {
        payment
            .expires_at
            .as_deref()
            .and_then(crate::payment::db_time)
            .unwrap_or_else(|| in_secs(QR_TTL_SECS))
    });
    Ok(Started {
        pay_ref: Some(payment.id),
        pay_state: payment.status,
        qr_payload,
        note: None,
        expires_at,
    })
}

async fn save_started(state: &AppState, order_id: i64, started: Started) -> ApiResult<()> {
    sqlx::query(
        "UPDATE orders SET pay_ref = ?, pay_state = ?, qr_payload = ?, pay_note = ?,          pay_expires_at = ? WHERE id = ?",
    )
    .bind(started.pay_ref)
    .bind(started.pay_state)
    .bind(started.qr_payload)
    .bind(started.note)
    .bind(started.expires_at)
    .bind(order_id)
    .execute(&state.pool)
    .await?;
    publish_pay(state, order_id).await
}

/// Cashier orders: when the verify API cannot be used the order still gets a plain QR that
/// the cashier confirms by hand (`pay_state = manual`, with the reason in `pay_note`).
pub async fn start_payment(state: &AppState, order_id: i64) -> ApiResult<()> {
    let order = view_by_id(state, order_id).await?;
    // A new QR for the same order must not queue behind the order's own old one.
    if let Some(old) = waiting_remote(state, order_id).await {
        cancel_remote(state, &old).await;
    }
    let pay = state.pay();
    let manual = |note: Option<String>| Started {
        pay_ref: None,
        pay_state: "manual".to_string(),
        qr_payload: static_qr(&pay.settings, order.total_satang),
        note,
        expires_at: None,
    };
    let started = match &pay.client {
        Some(client) => open_remote(client, &pay.settings, order.total_satang)
            .await
            .unwrap_or_else(|err| {
                tracing::error!(error = %format!("{err:#}"), order_id, "payment api failed, falling back to manual");
                manual(Some(err.to_string()))
            }),
        None => manual(None),
    };
    save_started(state, order_id, started).await
}

/// Fails unless a payment made now would really be watched: the verify API is set up and
/// its bot is not reported offline. Checked before a customer's order is created.
pub async fn ensure_verifiable(state: &AppState) -> ApiResult<()> {
    let pay = state.pay();
    let client = pay
        .client
        .as_ref()
        .ok_or(ApiError::Unavailable(PAY_UNAVAILABLE))?;
    match client.bot_health().await {
        Ok(health) if health.state == BotState::Offline => {
            tracing::warn!(status = ?health.status, enabled = health.enabled, "bot offline: self order refused");
            Err(ApiError::Unavailable(PAY_UNAVAILABLE))
        }
        Ok(_) => Ok(()),
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "bot status unknown: self order refused");
            Err(ApiError::Unavailable(PAY_UNAVAILABLE))
        }
    }
}

const QR_BUSY: &str = "มีลูกค้าท่านอื่นกำลังชำระเงินอยู่ กรุณารอสักครู่ (ไม่เกิน 2 นาที) แล้วกดลองใหม่อีกครั้ง";

/// One payable QR at a time for customers. While another QR is still waiting for its money (not
/// expired), a customer's order is refused, so two customers never pay into the same check and
/// nobody can scan the wrong QR. It frees itself when that QR is paid, expires, or the cashier
/// cancels the order or makes a new price. `except_order` is the order asking (its own QR
/// does not count against it).
pub async fn ensure_qr_free(state: &AppState, except_order: Option<i64>) -> ApiResult<()> {
    let busy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM orders WHERE shop_id = ? AND id <> ? \
         AND status = 'pending_payment' AND method = 'promptpay' \
         AND pay_state IN ('pending', 'queued') \
         AND (pay_expires_at > datetime('now') \
              OR (pay_state = 'queued' AND created_at >= datetime('now', '-5 minutes')))",
    )
    .bind(state.shop.id)
    .bind(except_order.unwrap_or(-1))
    .fetch_one(&state.pool)
    .await?;
    if busy > 0 {
        return Err(ApiError::Conflict(QR_BUSY.to_string()));
    }
    Ok(())
}

/// Customer orders: only a QR registered with the verify API is accepted, never the plain one.
pub async fn start_verified_payment(state: &AppState, order_id: i64) -> ApiResult<()> {
    let order = view_by_id(state, order_id).await?;
    if let Some(old) = waiting_remote(state, order_id).await {
        cancel_remote(state, &old).await;
    }
    let pay = state.pay();
    let client = pay
        .client
        .as_ref()
        .ok_or(ApiError::Unavailable(PAY_UNAVAILABLE))?;
    let started = open_remote(client, &pay.settings, order.total_satang)
        .await
        .map_err(|err| {
            tracing::error!(error = %format!("{err:#}"), order_id, "payment api failed for a customer order");
            ApiError::Unavailable(PAY_UNAVAILABLE)
        })?;
    if !matches!(started.pay_state.as_str(), "pending" | "queued") {
        tracing::error!(order_id, state = %started.pay_state, "payment api returned an unusable payment");
        if let Some(id) = &started.pay_ref {
            cancel_remote(state, id).await;
        }
        return Err(ApiError::Unavailable(PAY_UNAVAILABLE));
    }
    save_started(state, order_id, started).await
}

fn static_qr(settings: &PaySettings, total_satang: i64) -> Option<String> {
    promptpay::payload(&settings.promptpay_id, total_satang)
}

/// What the customer display needs to show the payment QR of an order.
fn pay_json(order: &OrderView) -> serde_json::Value {
    json!({
        "order_id": order.id,
        "total_satang": order.total_satang,
        "state": order.pay_state,
        "qr_svg": order.qr_svg,
        "expires_in": order.qr_expires_in,
        "source": order.source,
    })
}

/// The QR the customer display should be showing right now: the newest cashier order still
/// waiting for a PromptPay payment. Only recent ones, so a forgotten order does not linger.
pub async fn display_pay(state: &AppState) -> ApiResult<Option<serde_json::Value>> {
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM orders WHERE shop_id = ? AND status = 'pending_payment'          AND source = 'pos' AND method = 'promptpay'          AND created_at >= datetime('now', '-20 minutes') ORDER BY id DESC LIMIT 1",
    )
    .bind(state.shop.id)
    .fetch_optional(&state.pool)
    .await?;
    match id {
        Some(id) => Ok(Some(pay_json(&view_by_id(state, id).await?))),
        None => Ok(None),
    }
}

pub async fn publish_pay(state: &AppState, order_id: i64) -> ApiResult<()> {
    let order = view_by_id(state, order_id).await?;
    state.publish(Event::for_order("pay", &order.token, pay_json(&order)));
    state.publish(Event::for_order(
        "order",
        &order.token,
        json!({ "status": order.status }),
    ));
    state.publish(Event::new("orders", json!({})));
    Ok(())
}

pub struct ExternalPayment {
    pub id: String,
    pub bank: Option<String>,
    pub payer_name: Option<String>,
}

pub async fn mark_paid(
    state: &AppState,
    order_id: i64,
    external: Option<ExternalPayment>,
) -> ApiResult<bool> {
    // Confirmed by hand (not by the verify API): its payment must not keep waiting in the queue.
    let stale_remote = match external {
        Some(_) => None,
        None => waiting_remote(state, order_id).await,
    };
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let current: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT status, total_satang, method FROM orders WHERE id = ? AND shop_id = ?",
    )
    .bind(order_id)
    .bind(state.shop.id)
    .fetch_optional(&mut *tx)
    .await?;
    let (status, total, method) = current.ok_or(ApiError::NotFound)?;
    if status != "pending_payment" {
        return Ok(false);
    }
    let date = today();
    let next_no: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(queue_no), 0) + 1 FROM orders WHERE shop_id = ? AND queue_date = ?",
    )
    .bind(state.shop.id)
    .bind(&date)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE orders SET status = 'paid', queue_no = ?, queue_date = ?, \
         paid_at = datetime('now'), pay_state = 'succeeded' WHERE id = ?",
    )
    .bind(next_no)
    .bind(&date)
    .bind(order_id)
    .execute(&mut *tx)
    .await?;
    let verified = external.is_some();
    let (external_id, bank, payer) = match external {
        Some(e) => (Some(e.id), e.bank, e.payer_name),
        None => (None, None, None),
    };
    let (bank_shown, payer_shown) = (bank.clone(), payer.clone());
    let slip = (external_id.clone(), bank.clone(), payer.clone());
    sqlx::query(
        "INSERT INTO payments (shop_id, order_id, amount_satang, method, external_id, bank, payer_name) \
         VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(external_id) DO NOTHING",
    )
    .bind(state.shop.id)
    .bind(order_id)
    .bind(total)
    .bind(&method)
    .bind(external_id)
    .bind(bank)
    .bind(payer)
    .execute(&mut *tx)
    .await?;
    // The slip: a permanent copy of the payment, cash or PromptPay (who, which bank, how much,
    // what was bought) that stays even when orders are cleared, so the dashboard can show it for
    // any past day.
    {
        let (external_id, bank, payer) = slip;
        sqlx::query(
            "INSERT OR IGNORE INTO payment_slips (shop_id, order_id, queue_no, queue_date, \
             amount_satang, method, verified, payer_name, bank, external_id, items, \
             cash_received_satang) \
             SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, COALESCE((SELECT json_group_array(\
             json_object('name', name, 'qty', qty, 'price', unit_price_satang)) \
             FROM order_items WHERE order_id = ?), '[]'), \
             (SELECT cash_received_satang FROM orders WHERE id = ?)",
        )
        .bind(state.shop.id)
        .bind(order_id)
        .bind(next_no)
        .bind(&date)
        .bind(total)
        .bind(&method)
        .bind(i64::from(verified))
        .bind(payer)
        .bind(bank)
        .bind(external_id)
        .bind(order_id)
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    }
    // The sheets are queued in the same transaction that hands out the queue number, so the
    // print order can never differ from the queue order.
    let print_cfg = state.print();
    let kinds = if print_cfg.ready() {
        printing::kinds_for(&print_cfg)
    } else {
        Vec::new()
    };
    printq::enqueue(&mut tx, state.shop.id, order_id, &kinds).await?;
    tx.commit().await?;
    if !kinds.is_empty() {
        printq::changed(state);
    }

    let order = view_by_id(state, order_id).await?;
    let label = state.bill().queue_label(next_no);
    state.publish(Event::for_order(
        "paid",
        &order.token,
        json!({
            "order_id": order.id,
            "queue_no": next_no,
            "queue_label": label,
            "total_satang": total,
            "cash_received_satang": order.cash_received_satang,
            "change_satang": order.change_satang,
            "say": thai::payment_phrase(total),
            "source": order.source,
        }),
    ));
    // Cashier-only details: no token and not in the display/customer event lists, so the
    // payer's name never reaches the customer screen or a customer's phone.
    state.publish(Event::new(
        "paid_detail",
        json!({
            "order_id": order.id,
            "queue_label": label,
            "total_satang": total,
            "cash_received_satang": order.cash_received_satang,
            "change_satang": order.change_satang,
            "method": method,
            "verified": verified,
            "bank": bank_shown,
            "payer_name": payer_shown,
        }),
    ));
    state.publish(Event::for_order(
        "order",
        &order.token,
        json!({ "status": "paid" }),
    ));
    state.publish(Event::new("orders", json!({})));
    cancel_remote_in_background(state, stale_remote);
    Ok(true)
}

/// The earliest paid queue that is still waiting to be called, if it comes before `queue_no`.
pub async fn earlier_waiting(
    pool: &SqlitePool,
    shop_id: i64,
    date: &str,
    queue_no: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT MIN(queue_no) FROM orders WHERE shop_id = ? AND queue_date = ? \
         AND status = 'paid' AND queue_no < ?",
    )
    .bind(shop_id)
    .bind(date)
    .bind(queue_no)
    .fetch_one(pool)
    .await
}

/// Calls the queue that has waited longest (the lowest paid number).
pub async fn call_next(state: &AppState) -> ApiResult<i64> {
    let next: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM orders WHERE shop_id = ? AND queue_date = ? AND status = 'paid' \
         ORDER BY queue_no LIMIT 1",
    )
    .bind(state.shop.id)
    .bind(today())
    .fetch_optional(&state.pool)
    .await?;
    let (id,) = next.ok_or(ApiError::BadRequest("ยังไม่มีคิวที่รอเรียก"))?;
    call_order(state, id, true).await?;
    Ok(id)
}

/// Calling a queue ahead of an earlier waiting one needs `force` (the cashier confirmed).
pub async fn call_order(state: &AppState, order_id: i64, force: bool) -> ApiResult<()> {
    if !force {
        let target: Option<(String, Option<i64>)> = sqlx::query_as(
            "SELECT status, queue_no FROM orders WHERE id = ? AND shop_id = ? AND queue_date = ?",
        )
        .bind(order_id)
        .bind(state.shop.id)
        .bind(today())
        .fetch_optional(&state.pool)
        .await?;
        if let Some((status, Some(no))) = target
            && status == "paid"
            && let Some(first) = earlier_waiting(&state.pool, state.shop.id, &today(), no).await?
        {
            return Err(ApiError::Conflict(format!(
                "ยังมีคิว {} ที่ชำระแล้วและรอเรียกอยู่ก่อนหน้า",
                state.bill().queue_label(first)
            )));
        }
    }
    let updated = sqlx::query(
        "UPDATE orders SET status = 'called', called_at = datetime('now') \
         WHERE id = ? AND shop_id = ? AND status IN ('paid', 'called')",
    )
    .bind(order_id)
    .bind(state.shop.id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::BadRequest("เรียกคิวได้เฉพาะออเดอร์ที่ชำระแล้ว"));
    }
    let order = view_by_id(state, order_id).await?;
    let queue_no = order.queue_no.unwrap_or_default();
    let line_state = state.clone();
    tokio::spawn(async move { crate::line::notify_called(&line_state, order_id).await });
    state.publish(Event::for_order(
        "called",
        &order.token,
        json!({
            "order_id": order.id,
            "queue_no": queue_no,
            "queue_label": state.bill().queue_label(queue_no),
            "say": thai::queue_phrase(queue_no),
        }),
    ));
    state.publish(Event::for_order(
        "order",
        &order.token,
        json!({ "status": "called" }),
    ));
    state.publish(Event::new("orders", json!({})));
    Ok(())
}

pub async fn close_order(state: &AppState, order_id: i64, done: bool) -> ApiResult<()> {
    let (status, allowed) = if done {
        ("done", "('called', 'paid')")
    } else {
        ("cancelled", "('pending_payment', 'paid', 'called')")
    };
    let stale_remote = if done {
        None
    } else {
        waiting_remote(state, order_id).await
    };
    let sql = format!(
        "UPDATE orders SET status = ?, done_at = datetime('now') \
         WHERE id = ? AND shop_id = ? AND status IN {allowed}"
    );
    let updated = sqlx::query(AssertSqlSafe(sql))
        .bind(status)
        .bind(order_id)
        .bind(state.shop.id)
        .execute(&state.pool)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::BadRequest("สถานะออเดอร์ไม่รองรับการทำรายการนี้"));
    }
    cancel_remote_in_background(state, stale_remote);
    let order = view_by_id(state, order_id).await?;
    state.publish(Event::for_order(
        "order",
        &order.token,
        json!({ "status": order.status }),
    ));
    state.publish(Event::new("idle", json!({ "order_id": order_id })));
    state.publish(Event::new("orders", json!({})));
    Ok(())
}

/// Thai explanation of the verify API's `verificationIssue`.
fn issue_text(issue: &str) -> String {
    match issue {
        "empty_text" => "ข้อความแจ้งเงินเข้าว่างเปล่า".to_string(),
        "unsupported_bank_message" => "รูปแบบข้อความของธนาคารนี้ยังไม่รองรับ".to_string(),
        "kbank_amount_hidden" => "KBank ไม่แสดงยอดเงินในข้อความแจ้งเตือน".to_string(),
        other => format!("ตรวจยอดไม่ผ่าน ({other})"),
    }
}

/// The bank-reported amount must equal the order total; an absent amount is not a mismatch.
fn amount_matches(reported: Option<i64>, total: i64) -> bool {
    reported.is_none_or(|amount| amount == total)
}

/// An order waiting for payment, as the poller last saw it.
#[derive(sqlx::FromRow)]
struct Waiting {
    order_id: i64,
    pay_ref: String,
    total: i64,
    pay_state: String,
    pay_note: String,
}

async fn save_pay_state(
    state: &AppState,
    order_id: i64,
    status: &str,
    qr_payload: Option<&str>,
    note: &str,
    expires_at: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE orders SET pay_state = ?, qr_payload = COALESCE(?, qr_payload), \
         pay_note = NULLIF(?, ''), pay_expires_at = COALESCE(?, pay_expires_at) WHERE id = ?",
    )
    .bind(status)
    .bind(qr_payload)
    .bind(note)
    .bind(expires_at)
    .bind(order_id)
    .execute(&state.pool)
    .await?;
    publish_pay(state, order_id).await
}

async fn apply_remote(state: &AppState, waiting: &Waiting, remote: RemotePayment) {
    let order_id = waiting.order_id;
    let note = remote
        .verification_issue
        .as_deref()
        .map(issue_text)
        .unwrap_or_default();
    let outcome = match remote.status.as_str() {
        "succeeded" if !amount_matches(remote.amount_satang, waiting.total) => {
            tracing::warn!(order_id, reported = ?remote.amount_satang, total = waiting.total, "paid amount differs from order total");
            let reported = remote.amount_satang.unwrap_or_default();
            let note = format!(
                "ยอดที่ธนาคารแจ้ง {} บาท ไม่ตรงกับออเดอร์ {} บาท ตรวจในแอปธนาคารแล้วกดยืนยันเอง",
                format_baht(reported),
                format_baht(waiting.total)
            );
            save_pay_state(state, order_id, "ambiguous", None, &note, None).await
        }
        "succeeded" => {
            let external = ExternalPayment {
                id: remote.id.clone(),
                bank: remote.bank.clone(),
                payer_name: remote.payer_name.clone(),
            };
            mark_paid(state, order_id, Some(external)).await.map(|_| ())
        }
        status @ ("pending" | "queued" | "expired" | "cancelled" | "ambiguous") => {
            if status == waiting.pay_state && note == waiting.pay_note {
                return;
            }
            tracing::info!(order_id, status, issue = ?remote.verification_issue, "payment status changed");
            let expires_at = (status == "pending")
                .then(|| {
                    remote
                        .expires_at
                        .as_deref()
                        .and_then(crate::payment::db_time)
                })
                .flatten();
            save_pay_state(
                state,
                order_id,
                status,
                remote.qr_payload.as_deref(),
                &note,
                expires_at.as_deref(),
            )
            .await
        }
        _ => Ok(()),
    };
    if let Err(err) = outcome {
        match err {
            ApiError::Internal(e) => {
                tracing::error!(error = %e, order_id, "apply payment status failed")
            }
            _ => tracing::warn!(order_id, "apply payment status rejected"),
        }
    }
}

pub async fn run_poller(state: AppState) {
    if state.pay().client.is_none() {
        tracing::warn!("payment api not configured: running in manual-confirm mode");
    }
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let Some(client) = state.pay().client.clone() else {
            continue;
        };
        let rows: Result<Vec<Waiting>, _> = sqlx::query_as(
            "SELECT id AS order_id, pay_ref, total_satang AS total,              COALESCE(pay_state, '') AS pay_state, COALESCE(pay_note, '') AS pay_note              FROM orders              WHERE shop_id = ? AND status = 'pending_payment' AND pay_ref IS NOT NULL              AND pay_state IN ('queued', 'pending')",
        )
        .bind(state.shop.id)
        .fetch_all(&state.pool)
        .await;
        let Ok(rows) = rows else {
            tracing::error!("poller: could not list waiting orders");
            continue;
        };
        for waiting in rows {
            match client.get(&waiting.pay_ref).await {
                Ok(remote) => apply_remote(&state, &waiting, remote).await,
                Err(err) => {
                    let order_id = waiting.order_id;
                    tracing::warn!(error = %format!("{err:#}"), order_id, "poll failed")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_down_to_the_expiry_and_stops_at_zero() {
        let now = chrono::NaiveDateTime::parse_from_str("2026-11-22 09:00:00", "%Y-%m-%d %H:%M:%S")
            .unwrap();
        assert_eq!(remaining_secs(Some("2026-11-22 09:02:00"), now), Some(120));
        assert_eq!(remaining_secs(Some("2026-11-22 09:00:30"), now), Some(30));
        assert_eq!(remaining_secs(Some("2026-11-22 08:59:00"), now), Some(0));
        assert_eq!(remaining_secs(Some("2569-11-22 09:00:00"), now), Some(600));
        assert_eq!(remaining_secs(None, now), None);
        assert_eq!(remaining_secs(Some("garbage"), now), None);
    }

    #[test]
    fn amount_check_only_fails_on_a_real_difference() {
        assert!(amount_matches(None, 4_000));
        assert!(amount_matches(Some(4_000), 4_000));
        assert!(!amount_matches(Some(3_900), 4_000));
        assert!(!amount_matches(Some(0), 4_000));
    }

    #[test]
    fn explains_known_verification_issues_in_thai() {
        assert!(issue_text("kbank_amount_hidden").contains("KBank"));
        assert!(issue_text("empty_text").contains("ว่างเปล่า"));
        assert!(issue_text("something_new").contains("something_new"));
    }
}
