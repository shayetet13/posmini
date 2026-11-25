//! Tells a customer on LINE when their queue is called.
//!
//! The customer opens their queue page, taps "notify me on LINE", signs in through LIFF and the
//! page sends us the ID token. We check it with LINE (that is what proves whose LINE account it
//! is), keep the LINE user id on the order, and push a message when the cashier calls the queue.
//! The shop's own LINE Official Account sends it, so the customer must be its friend.

use std::collections::HashMap;
use std::time::Duration;

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    admin::clean_text,
    auth::Admin,
    error::{ApiError, ApiResult},
    settings,
    state::AppState,
};

const DEFAULT_BASE: &str = "https://api.line.me";
const TOKEN_MAX_CHARS: usize = 600;
const ID_MAX_CHARS: usize = 60;
const ID_TOKEN_MAX_CHARS: usize = 4000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LINK_MAX: usize = 10;
const LINK_WINDOW: Duration = Duration::from_secs(300);

const KEY_TOKEN: &str = "line_channel_token";
const KEY_LOGIN: &str = "line_login_channel_id";
const KEY_LIFF: &str = "line_liff_id";
/// Only ever set by hand in the database (the tests point it at a pretend LINE).
const KEY_BASE: &str = "line_api_base";

#[derive(Clone, Default)]
pub struct LineSettings {
    token: String,
    login_channel_id: String,
    liff_id: String,
    base: String,
}

impl LineSettings {
    fn from_map(map: &HashMap<String, String>) -> Self {
        let get = |key: &str| {
            map.get(key)
                .map(|v| v.trim().to_string())
                .unwrap_or_default()
        };
        // The environment variable lets an end-to-end test point the app at a pretend LINE.
        let base = Some(get(KEY_BASE))
            .filter(|b| !b.is_empty())
            .or_else(|| std::env::var("LINE_API_BASE").ok())
            .unwrap_or_default();
        let base = base.trim().to_string();
        Self {
            token: get(KEY_TOKEN),
            login_channel_id: get(KEY_LOGIN),
            liff_id: get(KEY_LIFF),
            base: if base.is_empty() {
                DEFAULT_BASE.to_string()
            } else {
                base.trim_end_matches('/').to_string()
            },
        }
    }

    /// All three values are needed: the token sends, the login channel verifies, LIFF signs in.
    pub fn ready(&self) -> bool {
        !self.token.is_empty() && !self.login_channel_id.is_empty() && !self.liff_id.is_empty()
    }
}

pub async fn load(state: &AppState) -> LineSettings {
    match settings::load_map(&state.pool, state.shop.id).await {
        Ok(map) => LineSettings::from_map(&map),
        Err(err) => {
            tracing::warn!(error = %err, "could not read the LINE settings");
            LineSettings::default()
        }
    }
}

fn view(s: &LineSettings) -> Value {
    json!({
        "liff_id": s.liff_id,
        "login_channel_id": s.login_channel_id,
        "token_set": !s.token.is_empty(),
        "ready": s.ready(),
    })
}

pub async fn get_settings(_admin: Admin, State(state): State<AppState>) -> Json<Value> {
    Json(view(&load(&state).await))
}

#[derive(Deserialize)]
pub struct SettingsReq {
    #[serde(default)]
    liff_id: String,
    #[serde(default)]
    login_channel_id: String,
    /// Blank keeps the stored token, so it never has to travel back to the browser.
    #[serde(default)]
    channel_token: String,
}

fn id_field(value: &str, error: &'static str) -> ApiResult<String> {
    let text = clean_text(value, ID_MAX_CHARS).ok_or(ApiError::BadRequest(error))?;
    let ok = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    ok.then_some(text).ok_or(ApiError::BadRequest(error))
}

pub async fn put_settings(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<SettingsReq>,
) -> ApiResult<Json<Value>> {
    let current = load(&state).await;
    let liff_id = id_field(&req.liff_id, "LIFF ID ไม่ถูกต้อง")?;
    let login = id_field(&req.login_channel_id, "Channel ID ของ LINE Login ไม่ถูกต้อง")?;
    let token = if req.channel_token.trim().is_empty() {
        current.token.clone()
    } else {
        clean_text(&req.channel_token, TOKEN_MAX_CHARS)
            .filter(|t| !t.contains(char::is_whitespace))
            .ok_or(ApiError::BadRequest("Channel access token ไม่ถูกต้อง"))?
    };
    settings::save_pairs(
        &state.pool,
        state.shop.id,
        &[(KEY_LIFF, liff_id), (KEY_LOGIN, login), (KEY_TOKEN, token)],
    )
    .await?;
    Ok(Json(view(&load(&state).await)))
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn http() -> ApiResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))
}

/// LINE checks the ID token's signature and expiry; its `sub` is the customer's LINE user id.
async fn verified_user(cfg: &LineSettings, id_token: &str) -> Option<String> {
    let body = format!(
        "id_token={}&client_id={}",
        percent_encode(id_token),
        percent_encode(&cfg.login_channel_id)
    );
    let reply = http()
        .ok()?
        .post(format!("{}/oauth2/v2.1/verify", cfg.base))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .ok()?;
    if !reply.status().is_success() {
        return None;
    }
    let found: Value = reply.json().await.ok()?;
    found["sub"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// What the queue page needs to offer the button.
pub async fn offer(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> ApiResult<Json<Value>> {
    crate::api::check_token(&token)?;
    let linked: Option<Option<String>> =
        sqlx::query_scalar("SELECT line_user_id FROM orders WHERE shop_id = ? AND token = ?")
            .bind(state.shop.id)
            .bind(&token)
            .fetch_optional(&state.pool)
            .await?;
    let linked = linked.ok_or(ApiError::NotFound)?;
    let cfg = load(&state).await;
    Ok(Json(json!({
        "enabled": cfg.ready(),
        "liff_id": if cfg.ready() { cfg.liff_id.as_str() } else { "" },
        "linked": linked.is_some(),
    })))
}

#[derive(Deserialize)]
pub struct LinkReq {
    id_token: String,
}

pub async fn link(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(req): Json<LinkReq>,
) -> ApiResult<Json<Value>> {
    crate::api::check_token(&token)?;
    // Unknown tokens are refused before anything is counted, so junk cannot fill the limiter.
    let known: Option<i64> =
        sqlx::query_scalar("SELECT id FROM orders WHERE shop_id = ? AND token = ?")
            .bind(state.shop.id)
            .bind(&token)
            .fetch_optional(&state.pool)
            .await?;
    known.ok_or(ApiError::NotFound)?;
    let key = format!("line-link:{token}");
    if state.is_limited(&key, LINK_MAX, LINK_WINDOW) {
        return Err(ApiError::TooMany);
    }
    state.record_hit(&key);
    if req.id_token.is_empty() || req.id_token.chars().count() > ID_TOKEN_MAX_CHARS {
        return Err(ApiError::BadRequest("ข้อมูลจาก LINE ไม่ถูกต้อง"));
    }
    let cfg = load(&state).await;
    if !cfg.ready() {
        return Err(ApiError::BadRequest("ร้านนี้ยังไม่เปิดการแจ้งเตือนทาง LINE"));
    }
    let user = verified_user(&cfg, &req.id_token)
        .await
        .ok_or(ApiError::BadRequest("ยืนยันบัญชี LINE ไม่สำเร็จ ลองใหม่อีกครั้ง"))?;
    let saved = sqlx::query(
        "UPDATE orders SET line_user_id = ? WHERE shop_id = ? AND token = ? \
         AND status IN ('pending_payment', 'paid', 'called')",
    )
    .bind(&user)
    .bind(state.shop.id)
    .bind(&token)
    .execute(&state.pool)
    .await?;
    if saved.rows_affected() == 0 {
        return Err(ApiError::BadRequest("ออเดอร์นี้ปิดไปแล้ว"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Pushes "your queue is called" to the customer's LINE, if they asked for it. Never fails the
/// call: a missed message is logged, and the screen, the voice and the staff still call the queue.
pub async fn notify_called(state: &AppState, order_id: i64) {
    let found: Result<Option<(Option<String>, Option<i64>)>, _> =
        sqlx::query_as("SELECT line_user_id, queue_no FROM orders WHERE id = ? AND shop_id = ?")
            .bind(order_id)
            .bind(state.shop.id)
            .fetch_optional(&state.pool)
            .await;
    let Ok(Some((Some(user), queue_no))) = found else {
        return;
    };
    let cfg = load(state).await;
    if !cfg.ready() {
        return;
    }
    let bill = state.bill();
    let label = queue_no.map(|no| bill.queue_label(no)).unwrap_or_default();
    let text = format!(
        "{} · คิว {label} ถึงคิวแล้ว มารับสินค้าได้เลยค่ะ",
        bill.shop_title(&state.shop.name)
    );
    let sent = match http() {
        Ok(client) => {
            client
                .post(format!("{}/v2/bot/message/push", cfg.base))
                .bearer_auth(&cfg.token)
                .json(&json!({ "to": user, "messages": [{ "type": "text", "text": text }] }))
                .send()
                .await
        }
        Err(_) => return,
    };
    match sent {
        Ok(reply) if reply.status().is_success() => {}
        Ok(reply) => tracing::warn!(order_id, status = %reply.status(), "LINE push was refused"),
        Err(err) => tracing::warn!(order_id, error = %err, "LINE push failed"),
    }
}
