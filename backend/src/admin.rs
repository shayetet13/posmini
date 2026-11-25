use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    audit,
    auth::{Admin, Authed, Role},
    error::{ApiError, ApiResult},
    events::Event,
    menu,
    payment::{BotHealth, BotState, PingStatus, Route},
    settings::{self, PaySettings, digits_of},
    state::{AppState, PayRuntime},
};

pub const IMAGE_MAX_BYTES: usize = 1_048_576;
const NAME_MAX_CHARS: usize = 60;
const CATEGORY_MAX_CHARS: usize = 30;
const PRICE_MAX_SATANG: i64 = 10_000_000;
const FIELD_MAX_CHARS: usize = 200;
const BULK_MAX_IDS: usize = 500;
/// Groups inside groups stop here, so a loop in the data can never hang a request.
const MENU_DEPTH_LIMIT: usize = 8;

// ---------------------------------------------------------------- products

#[derive(Deserialize)]
pub struct ProductReq {
    name: String,
    category: String,
    price_satang: i64,
    /// The group this row sits in. Left out = "not said" (an older page that knows nothing about
    /// groups must not move a row); `null` = the top level; a number = that group.
    #[serde(default, deserialize_with = "said_or_null")]
    parent_id: Option<Option<i64>>,
    /// A group holds other rows and has no price. Fixed when the row is created.
    #[serde(default)]
    is_group: bool,
    /// Card colour `#RRGGBB`; left out, a free one is chosen.
    #[serde(default)]
    color: Option<String>,
}

struct ProductInput {
    name: String,
    category: String,
    price_satang: i64,
    parent_id: Option<Option<i64>>,
    is_group: bool,
    color: Option<String>,
}

/// Tells a field that was left out (`None`) from one sent as `null` (`Some(None)`).
fn said_or_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(d).map(Some)
}

pub fn clean_text(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    let ok = trimmed.chars().count() <= max_chars && !trimmed.chars().any(char::is_control);
    ok.then(|| trimmed.to_string())
}

fn product_input(req: ProductReq) -> ApiResult<ProductInput> {
    let name = clean_text(&req.name, NAME_MAX_CHARS)
        .filter(|n| !n.is_empty())
        .ok_or(ApiError::BadRequest("ชื่อสินค้าต้องมี 1-60 ตัวอักษร"))?;
    let category = clean_text(&req.category, CATEGORY_MAX_CHARS)
        .ok_or(ApiError::BadRequest("ชื่อหมวดยาวเกิน 30 ตัวอักษร"))?;
    // A group has no price of its own; only what is inside it is sold.
    let price_satang = if req.is_group {
        0
    } else if (1..=PRICE_MAX_SATANG).contains(&req.price_satang) {
        req.price_satang
    } else {
        return Err(ApiError::BadRequest("ราคาต้องมากกว่า 0 และไม่เกิน 100,000 บาท"));
    };
    let color = match req
        .color
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        Some(text) => Some(
            menu::normal_color(text)
                .ok_or(ApiError::BadRequest("สีต้องเป็นรหัส #RRGGBB เช่น #E5484D"))?,
        ),
        None => None,
    };
    Ok(ProductInput {
        name,
        category,
        price_satang,
        parent_id: req.parent_id,
        is_group: req.is_group,
        color,
    })
}

/// The group a row is put into must exist, be a group, and not be the row itself or inside it
/// (a group cannot end up under its own child).
async fn check_parent(state: &AppState, own_id: Option<i64>, parent: Option<i64>) -> ApiResult<()> {
    let bad_group = || ApiError::BadRequest("เลือกกลุ่มที่ถูกต้อง");
    let mut current = parent;
    let mut hops = 0;
    while let Some(id) = current {
        if own_id == Some(id) || hops > MENU_DEPTH_LIMIT {
            return Err(bad_group());
        }
        let row: Option<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT is_group, parent_id FROM products WHERE id = ? AND shop_id = ? AND active = 1",
        )
        .bind(id)
        .bind(state.shop.id)
        .fetch_optional(&state.pool)
        .await?;
        match row {
            Some((1, next)) => current = next,
            _ => return Err(bad_group()),
        }
        hops += 1;
    }
    Ok(())
}

/// The colour for a row: the one asked for if no neighbour has it, else a refusal. When none is
/// asked for, the row keeps its current colour (`keep`) unless a neighbour already has it (the
/// row was moved into another group): then it gets a free one instead of an error.
async fn color_for(
    state: &AppState,
    own_id: Option<i64>,
    parent: Option<i64>,
    asked: Option<&str>,
    keep: Option<&str>,
) -> ApiResult<String> {
    let rows = menu::siblings(&state.pool, state.shop.id, parent).await?;
    let others = || rows.iter().filter(|(id, _, _)| Some(*id) != own_id);
    let taken_by = |color: &str| {
        others()
            .find(|(_, _, c)| c.as_deref() == Some(color))
            .map(|(_, name, _)| name.clone())
    };
    if let Some(want) = asked {
        if let Some(name) = taken_by(want) {
            return Err(ApiError::Conflict(format!(
                "สีนี้ใช้กับ “{name}” อยู่แล้ว เลือกสีอื่นเพื่อให้จำง่าย"
            )));
        }
        return Ok(want.to_string());
    }
    if let Some(kept) = keep.filter(|c| taken_by(c).is_none()) {
        return Ok(kept.to_string());
    }
    let used = others().filter_map(|(_, _, c)| c.clone()).collect();
    Ok(menu::pick_color(&used))
}

fn products_changed(state: &AppState) {
    state.publish(Event::new("products", json!({})));
}

pub async fn create_product(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<ProductReq>,
) -> ApiResult<Json<Value>> {
    let input = product_input(req)?;
    let parent = input.parent_id.flatten();
    check_parent(&state, None, parent).await?;
    let color = color_for(&state, None, parent, input.color.as_deref(), None).await?;
    // Inside a group only the group's own list matters; the top-level category is for the top.
    let category = if parent.is_some() {
        ""
    } else {
        input.category.as_str()
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO products (shop_id, name, category, price_satang, parent_id, is_group, color, sort) \
         VALUES (?, ?, ?, ?, ?, ?, ?, (SELECT COALESCE(MAX(sort), -1) + 1 FROM products WHERE shop_id = ?)) \
         RETURNING id",
    )
    .bind(state.shop.id)
    .bind(&input.name)
    .bind(category)
    .bind(input.price_satang)
    .bind(parent)
    .bind(i64::from(input.is_group))
    .bind(&color)
    .bind(state.shop.id)
    .fetch_one(&state.pool)
    .await?;
    products_changed(&state);
    Ok(Json(json!({ "id": id })))
}

pub async fn update_product(
    _admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ProductReq>,
) -> ApiResult<Json<Value>> {
    let input = product_input(req)?;
    // Whether a row is a group is decided when it is created and never changes.
    let current: (i64, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT is_group, color, parent_id FROM products WHERE id = ? AND shop_id = ? AND active = 1",
    )
    .bind(id)
    .bind(state.shop.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    if input.is_group != (current.0 == 1) {
        return Err(ApiError::BadRequest(
            "เปลี่ยนสินค้าเป็นกลุ่ม (หรือกลับกัน) ภายหลังไม่ได้",
        ));
    }
    // Not said = stays where it is.
    let parent = input.parent_id.unwrap_or(current.2);
    check_parent(&state, Some(id), parent).await?;
    let color = color_for(
        &state,
        Some(id),
        parent,
        input.color.as_deref(),
        current.1.as_deref(),
    )
    .await?;
    let category = if parent.is_some() {
        ""
    } else {
        input.category.as_str()
    };
    let updated = sqlx::query(
        "UPDATE products SET name = ?, category = ?, price_satang = ?, parent_id = ?, color = ? \
         WHERE id = ? AND shop_id = ? AND active = 1",
    )
    .bind(&input.name)
    .bind(category)
    .bind(input.price_satang)
    .bind(parent)
    .bind(&color)
    .bind(id)
    .bind(state.shop.id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    products_changed(&state);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ReorderReq {
    /// The group whose rows are being put in order (`None` = the top level).
    #[serde(default)]
    parent_id: Option<i64>,
    /// Every row of that group, in the new order.
    ids: Vec<i64>,
}

/// Puts the rows of one group (or the top level) in the order the settings page dragged them to.
/// The list must be exactly the rows that are there now, so a stale page cannot drop or
/// duplicate anything.
pub async fn reorder_products(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<ReorderReq>,
) -> ApiResult<Json<Value>> {
    if req.ids.is_empty() || req.ids.len() > BULK_MAX_IDS {
        return Err(ApiError::BadRequest("ลำดับเมนูไม่ถูกต้อง"));
    }
    let current: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM products WHERE shop_id = ? AND active = 1 AND parent_id IS ?",
    )
    .bind(state.shop.id)
    .bind(req.parent_id)
    .fetch_all(&state.pool)
    .await?;
    let want: HashSet<i64> = req.ids.iter().copied().collect();
    let have: HashSet<i64> = current.into_iter().collect();
    if want.len() != req.ids.len() || want != have {
        return Err(ApiError::Conflict(
            "เมนูถูกเปลี่ยนจากเครื่องอื่น รีเฟรชหน้านี้แล้วจัดลำดับใหม่".to_string(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    for (position, id) in (0_i64..).zip(&req.ids) {
        sqlx::query("UPDATE products SET sort = ? WHERE id = ? AND shop_id = ?")
            .bind(position)
            .bind(id)
            .bind(state.shop.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    products_changed(&state);
    Ok(Json(json!({ "ok": true })))
}

/// Soft delete: past orders keep their own copy of name and price.
pub async fn delete_product(
    _admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let updated = sqlx::query(SOFT_DELETE_TREE_SQL)
        .bind(id)
        .bind(state.shop.id)
        .bind(state.shop.id)
        .execute(&state.pool)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    products_changed(&state);
    Ok(Json(json!({ "ok": true })))
}

/// Removes a row and, when it is a group, everything inside it (binds: id, shop, shop).
const SOFT_DELETE_TREE_SQL: &str = "WITH RECURSIVE sub(id) AS ( \
     SELECT id FROM products WHERE id = ? AND shop_id = ? AND active = 1 \
     UNION ALL \
     SELECT p.id FROM products p JOIN sub ON p.parent_id = sub.id \
     WHERE p.shop_id = ? AND p.active = 1) \
     UPDATE products SET active = 0, image_mime = NULL, image_data = NULL, \
     image_v = image_v + 1 WHERE id IN (SELECT id FROM sub)";

const SOFT_DELETE_SQL: &str = "UPDATE products SET active = 0, image_mime = NULL, \
     image_data = NULL, image_v = image_v + 1 WHERE shop_id = ? AND active = 1";

#[derive(Deserialize)]
pub struct BulkDeleteReq {
    ids: Vec<i64>,
}

/// Deletes the selected products; unknown or already-deleted ids are ignored.
pub async fn bulk_delete_products(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<BulkDeleteReq>,
) -> ApiResult<Json<Value>> {
    if req.ids.is_empty() || req.ids.len() > BULK_MAX_IDS {
        return Err(ApiError::BadRequest("เลือกสินค้าที่จะลบก่อน"));
    }
    let mut tx = state.pool.begin().await?;
    let mut deleted = 0;
    for id in &req.ids {
        let done = sqlx::query(SOFT_DELETE_TREE_SQL)
            .bind(id)
            .bind(state.shop.id)
            .bind(state.shop.id)
            .execute(&mut *tx)
            .await?;
        deleted += done.rows_affected();
    }
    tx.commit().await?;
    products_changed(&state);
    Ok(Json(json!({ "deleted": deleted })))
}

pub async fn delete_all_products(
    _admin: Admin,
    State(state): State<AppState>,
) -> ApiResult<Json<Value>> {
    let done = sqlx::query(SOFT_DELETE_SQL)
        .bind(state.shop.id)
        .execute(&state.pool)
        .await?;
    products_changed(&state);
    Ok(Json(json!({ "deleted": done.rows_affected() })))
}

fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub async fn set_image(
    _admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    if body.len() > IMAGE_MAX_BYTES {
        return Err(ApiError::BadRequest("รูปใหญ่เกินไป (สูงสุด 1 MB)"));
    }
    let mime = sniff_image(&body).ok_or(ApiError::BadRequest("รองรับเฉพาะรูป JPEG, PNG, WebP"))?;
    let updated = sqlx::query(
        "UPDATE products SET image_mime = ?, image_data = ?, image_v = image_v + 1 \
         WHERE id = ? AND shop_id = ? AND active = 1",
    )
    .bind(mime)
    .bind(body.as_ref())
    .bind(id)
    .bind(state.shop.id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    products_changed(&state);
    Ok(Json(json!({ "ok": true })))
}

pub async fn clear_image(
    _admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let updated = sqlx::query(
        "UPDATE products SET image_mime = NULL, image_data = NULL, image_v = image_v + 1 \
         WHERE id = ? AND shop_id = ? AND active = 1",
    )
    .bind(id)
    .bind(state.shop.id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    products_changed(&state);
    Ok(Json(json!({ "ok": true })))
}

pub async fn image(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    let row: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT image_mime, image_data FROM products \
         WHERE id = ? AND shop_id = ? AND active = 1 \
         AND image_mime IS NOT NULL AND image_data IS NOT NULL",
    )
    .bind(id)
    .bind(state.shop.id)
    .fetch_optional(&state.pool)
    .await?;
    let (mime, data) = row.ok_or(ApiError::NotFound)?;
    let content_type = match mime.as_str() {
        "image/png" => "image/png",
        "image/webp" => "image/webp",
        _ => "image/jpeg",
    };
    let mut response = data.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

// ---------------------------------------------------------------- settings

#[derive(Deserialize)]
pub struct SettingsReq {
    base_url: String,
    promptpay_id: String,
    dest_last4: String,
    /// Blank keeps the stored key, so the key never has to travel back to the browser.
    #[serde(default)]
    api_key: String,
    /// Optional; blank means "find it through the API".
    #[serde(default)]
    bot_id: String,
    #[serde(default)]
    source_target_id: String,
}

fn text_field(value: &str) -> ApiResult<String> {
    clean_text(value, FIELD_MAX_CHARS).ok_or(ApiError::BadRequest(
        "ข้อความในช่องตั้งค่ายาวเกินไปหรือมีอักขระที่ไม่ถูกต้อง",
    ))
}

fn settings_from_req(req: SettingsReq, current: &PaySettings) -> ApiResult<PaySettings> {
    let base_url = text_field(&req.base_url)?.trim_end_matches('/').to_string();
    let url_ok = base_url.is_empty()
        || ((base_url.starts_with("https://") || base_url.starts_with("http://"))
            && !base_url.contains(char::is_whitespace));
    if !url_ok {
        return Err(ApiError::BadRequest(
            "ที่อยู่ API ต้องขึ้นต้นด้วย https:// และห้ามมีช่องว่าง",
        ));
    }

    let raw_id = text_field(&req.promptpay_id)?;
    let id_chars_ok = raw_id
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '-'));
    let promptpay_id = digits_of(&raw_id);
    let candidate = PaySettings {
        promptpay_id: promptpay_id.clone(),
        ..PaySettings::default()
    };
    if !id_chars_ok || (!promptpay_id.is_empty() && candidate.id_type().is_none()) {
        return Err(ApiError::BadRequest(
            "เลขพร้อมเพย์ต้องเป็นเบอร์โทร 10 หลัก (ขึ้นต้น 0), เลขบัตรประชาชน 13 หลัก หรือ e-Wallet 15 หลัก",
        ));
    }

    let dest_last4 = text_field(&req.dest_last4)?;
    if !dest_last4.is_empty()
        && !(dest_last4.len() == 4 && dest_last4.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(ApiError::BadRequest("เลขท้ายบัญชีต้องเป็นตัวเลข 4 หลัก"));
    }

    let api_key = if req.api_key.trim().is_empty() {
        current.api_key.clone()
    } else {
        text_field(&req.api_key)?
    };

    Ok(PaySettings {
        base_url,
        api_key,
        promptpay_id,
        dest_last4,
        bot_id: text_field(&req.bot_id)?,
        source_target_id: text_field(&req.source_target_id)?,
    })
}

fn settings_view(runtime: &PayRuntime) -> Value {
    let s = &runtime.settings;
    json!({
        "base_url": s.base_url,
        "promptpay_id": s.promptpay_id,
        "dest_last4": s.dest_last4,
        "bot_id": s.bot_id,
        "source_target_id": s.source_target_id,
        "api_key_set": !s.api_key.is_empty(),
        "id_type": s.id_type(),
        "verify_active": runtime.client.is_some(),
    })
}

/// The admin sees every field; a cashier only learns whether automatic checking is on.
pub async fn get_settings(Authed(user): Authed, State(state): State<AppState>) -> Json<Value> {
    let view = settings_view(&state.pay());
    if user.role == Role::Admin {
        Json(view)
    } else {
        Json(json!({ "verify_active": view["verify_active"] }))
    }
}

pub async fn put_settings(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<SettingsReq>,
) -> ApiResult<Json<Value>> {
    let next = settings_from_req(req, &state.pay().settings)?;
    let runtime = PayRuntime::build(next.clone())?;
    settings::save(&state.pool, state.shop.id, &next).await?;
    let view = settings_view(&runtime);
    state.set_pay(runtime);
    Ok(Json(view))
}

fn is_auth_error(status: u16) -> bool {
    matches!(status, 401 | 403)
}

/// Turns the two probe statuses into a verdict. The key is only called good when the server
/// rejects the request without it but accepts it with it.
fn classify_ping(ping: &PingStatus) -> (bool, String) {
    let PingStatus {
        with_key,
        without_key,
    } = *ping;
    if is_auth_error(with_key) {
        (
            false,
            format!("API key ไม่ถูกต้อง หรือไม่มีสิทธิ์ใช้งาน (HTTP {with_key})"),
        )
    } else if with_key >= 500 {
        (false, format!("เซิร์ฟเวอร์ API มีปัญหา (HTTP {with_key})"))
    } else if is_auth_error(without_key) {
        (
            true,
            format!(
                "ติดต่อ API ได้ และ API key ใช้ได้ (ทดสอบด้วยรายการสมมติ จึงตอบ HTTP {with_key} ซึ่งเป็นเรื่องปกติ)"
            ),
        )
    } else {
        (
            false,
            format!(
                "ติดต่อเซิร์ฟเวอร์ได้ (HTTP {with_key}) แต่ยืนยัน key ไม่ได้ เพราะเซิร์ฟเวอร์ตอบเหมือนกันทั้งมีและไม่มี key ตรวจที่อยู่ API ให้ตรงกับเอกสาร"
            ),
        )
    }
}

/// Probes the verify API (no payment is created) to check the address and the key.
pub async fn test_settings(_admin: Admin, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let client = state.pay().client.clone().ok_or(ApiError::BadRequest(
        "ยังกรอกข้อมูลระบบตรวจยอดไม่ครบ กรุณาบันทึกให้ครบก่อน",
    ))?;
    let (mut ok, mut message) = match client.ping().await {
        Ok(ping) => classify_ping(&ping),
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "settings test: api unreachable");
            (false, err.to_string())
        }
    };
    if ok {
        match client.bot_health().await {
            Ok(health) => {
                let (_, bot_ok, text) = bot_verdict(&health);
                ok = bot_ok;
                message = format!(
                    "{message}
{text}"
                );
            }
            Err(err) => {
                message = format!(
                    "{message}
เช็กสถานะบอทไม่ได้: {err}"
                )
            }
        }
    }
    if ok {
        match client.route().await {
            Ok(route) => message = format!("{message}\n{}", describe_route(route)),
            Err(err) => {
                tracing::warn!(error = %format!("{err:#}"), "settings test: route lookup failed");
                ok = false;
                message = format!("API key ใช้ได้ แต่หาบอท/แหล่งแจ้งเงินเข้าไม่ได้: {err}");
            }
        }
    }
    Ok(Json(json!({ "ok": ok, "message": message })))
}

/// The bot's state as `(code, ok, Thai message)` for the settings page.
fn bot_verdict(health: &BotHealth) -> (&'static str, bool, String) {
    let who = health.name.as_deref().unwrap_or("บอท");
    let status = health.status.as_deref().unwrap_or("-");
    match health.state {
        BotState::Online => (
            "online",
            true,
            format!("บอท {who} ทำงานอยู่ (สถานะ: {status})"),
        ),
        BotState::Offline if !health.enabled => (
            "offline",
            false,
            format!("บอท {who} ถูกปิดใช้งานใน API จึงตรวจยอดเงินเข้าไม่ได้"),
        ),
        BotState::Offline => (
            "offline",
            false,
            format!(
                "บอท {who} ไม่ทำงาน (สถานะ: {status}) ลูกค้าจะสแกนจ่ายแล้วระบบไม่ตัดยอดให้ ตรวจสอบมือถือ/แอปที่รันบอทว่าเปิดอยู่และมีอินเทอร์เน็ต"
            ),
        ),
        BotState::Unknown => (
            "unknown",
            true,
            format!("บอท {who} เปิดใช้งานอยู่ แต่ API ไม่บอกสถานะการทำงาน (สถานะ: {status})"),
        ),
    }
}

/// Asks the verify API whether the bot is running right now (creates nothing).
async fn check_bot(state: &AppState) -> Value {
    let Some(client) = state.pay().client.clone() else {
        return json!({
            "state": "not_configured",
            "ok": false,
            "message": "ยังกรอกข้อมูลระบบตรวจยอดไม่ครบ จึงเช็กบอทไม่ได้",
        });
    };
    let (code, ok, message) = match client.bot_health().await {
        Ok(health) => bot_verdict(&health),
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "bot status check failed");
            ("error", false, format!("เช็กสถานะบอทไม่ได้: {err}"))
        }
    };
    json!({ "state": code, "ok": ok, "message": message })
}

/// Whether the bot that reads bank notifications is running (a fresh check, for the settings page).
pub async fn bot_status(_admin: Admin, State(state): State<AppState>) -> Json<Value> {
    Json(check_bot(&state).await)
}

/// How long the till may reuse an answer, so many tills do not each ask the verify API.
const PAY_HEALTH_TTL: std::time::Duration = std::time::Duration::from_secs(20);

/// For the till's warning banner. `mode` is "manual" when the shop never set automatic checking
/// up (nothing to warn about) and "auto" when it did; then `ok` says whether it works right now.
pub async fn pay_health(Authed(_): Authed, State(state): State<AppState>) -> Json<Value> {
    if let Some(cached) = state.cached_pay_health(PAY_HEALTH_TTL) {
        return Json(cached);
    }
    let mut body = check_bot(&state).await;
    let auto = state.pay().client.is_some();
    body["mode"] = json!(if auto { "auto" } else { "manual" });
    state.store_pay_health(body.clone());
    Json(body)
}

/// The word the settings page sends along, so a stray request can never wipe the sales.
pub const RESET_SALES_CONFIRM: &str = "reset-sales";

#[derive(Deserialize)]
pub struct ResetSalesReq {
    confirm: String,
    /// The admin's own password: a stray click or a borrowed screen cannot wipe the day.
    #[serde(default)]
    password: String,
}

const RESET_TRIES: usize = 5;
const RESET_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// Starts the shop over for sales: every order, payment and print job is deleted and the queue
/// starts again at the first number. The menu and every setting stay as they are.
pub async fn reset_sales(
    Admin(admin): Admin,
    State(state): State<AppState>,
    Json(req): Json<ResetSalesReq>,
) -> ApiResult<Json<Value>> {
    if req.confirm != RESET_SALES_CONFIRM {
        return Err(ApiError::BadRequest("ต้องยืนยันก่อนล้างยอดขาย"));
    }
    let tries = format!("reset:{}", admin.id);
    if state.is_limited(&tries, RESET_TRIES, RESET_WINDOW) {
        return Err(ApiError::TooMany);
    }
    if !crate::auth::password_matches(&state, admin.id, &req.password).await? {
        state.record_hit(&tries);
        return Err(ApiError::BadRequest("รหัสผ่านแอดมินไม่ถูกต้อง"));
    }
    let shop = state.shop.id;
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM print_jobs WHERE shop_id = ?")
        .bind(shop)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM payments WHERE shop_id = ?")
        .bind(shop)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM order_items WHERE order_id IN (SELECT id FROM orders WHERE shop_id = ?)",
    )
    .bind(shop)
    .execute(&mut *tx)
    .await?;
    let removed = sqlx::query("DELETE FROM orders WHERE shop_id = ?")
        .bind(shop)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    tracing::warn!(orders = removed, "sales reset from the settings page");
    let detail = format!("ล้างออเดอร์ {removed} รายการ");
    audit::record(&state, &admin, audit::action::RESET_SALES, None, &detail).await;

    // Every open screen reloads its lists; the print queue shows empty again.
    state.publish(Event::new("orders", json!({})));
    state.publish(Event::new("refresh", json!({})));
    crate::printq::publish(&state).await;
    Ok(Json(json!({ "ok": true, "orders": removed })))
}

/// The bots (and their chats) the verify API has, to choose from in settings.
pub async fn list_routes(_admin: Admin, State(state): State<AppState>) -> Json<Value> {
    let Some(client) = state.pay().client.clone() else {
        return Json(json!({
            "ok": false,
            "message": "ยังกรอกข้อมูลระบบตรวจยอดไม่ครบ (ที่อยู่ API, API key, เลขพร้อมเพย์) จึงดึงรายชื่อบอทไม่ได้",
        }));
    };
    match client.bots_with_targets().await {
        Ok(bots) => Json(json!({ "ok": true, "bots": bots })),
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "bot list failed");
            Json(json!({ "ok": false, "message": format!("ดึงรายชื่อบอทไม่ได้: {err}") }))
        }
    }
}

fn describe_route(route: &Route) -> String {
    let bot = route.bot_name.as_deref().unwrap_or(&route.bot_id);
    let target = route.target_name.as_deref().unwrap_or(&route.target_id);
    format!("บอท: {bot} · แหล่งแจ้งเงินเข้า: {target}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str) -> SettingsReq {
        SettingsReq {
            base_url: "https://api.example.com/".into(),
            promptpay_id: id.into(),
            dest_last4: String::new(),
            api_key: String::new(),
            bot_id: String::new(),
            source_target_id: String::new(),
        }
    }

    fn health(state: BotState, enabled: bool) -> BotHealth {
        BotHealth {
            state,
            name: Some("kao".into()),
            status: Some("offline".into()),
            enabled,
        }
    }

    #[test]
    fn explains_the_bot_state_to_the_cashier() {
        let (code, ok, text) = bot_verdict(&health(BotState::Online, true));
        assert_eq!((code, ok), ("online", true));
        assert!(text.contains("kao"));
        let (code, ok, text) = bot_verdict(&health(BotState::Offline, true));
        assert_eq!((code, ok), ("offline", false));
        assert!(text.contains("ไม่ทำงาน"));
        let (code, ok, text) = bot_verdict(&health(BotState::Offline, false));
        assert_eq!((code, ok), ("offline", false));
        assert!(text.contains("ปิดใช้งาน"));
        let (code, ok, _) = bot_verdict(&health(BotState::Unknown, true));
        assert_eq!((code, ok), ("unknown", true));
    }

    fn current() -> PaySettings {
        PaySettings {
            api_key: "lbk_saved".into(),
            ..PaySettings::default()
        }
    }

    #[test]
    fn keeps_saved_key_when_blank_and_normalizes() {
        let s = settings_from_req(req("081-234-5678"), &current())
            .ok()
            .unwrap();
        assert_eq!(s.api_key, "lbk_saved");
        assert_eq!(s.promptpay_id, "0812345678");
        assert_eq!(s.base_url, "https://api.example.com");
        assert!(s.verify_ready());
    }

    #[test]
    fn replaces_key_when_provided() {
        let mut r = req("0812345678");
        r.api_key = " lbk_new ".into();
        let s = settings_from_req(r, &current()).ok().unwrap();
        assert_eq!(s.api_key, "lbk_new");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(settings_from_req(req("12345"), &current()).is_err());
        assert!(settings_from_req(req("08x1234567"), &current()).is_err());
        let mut r = req("0812345678");
        r.base_url = "ftp://x".into();
        assert!(settings_from_req(r, &current()).is_err());
        let mut r = req("0812345678");
        r.dest_last4 = "12a4".into();
        assert!(settings_from_req(r, &current()).is_err());
    }

    #[test]
    fn classifies_ping_results() {
        let verdict = |with_key, without_key| {
            classify_ping(&PingStatus {
                with_key,
                without_key,
            })
        };
        assert!(verdict(404, 401).0, "key checked and accepted");
        assert!(verdict(200, 403).0);
        assert!(!verdict(401, 401).0, "key rejected");
        assert!(!verdict(404, 404).0, "key never checked: cannot confirm");
        assert!(!verdict(502, 502).0, "server error");
    }

    fn product_req(name: &str, category: &str, price_satang: i64) -> ProductReq {
        ProductReq {
            name: name.into(),
            category: category.into(),
            price_satang,
            parent_id: None,
            is_group: false,
            color: None,
        }
    }

    #[test]
    fn product_validation() {
        let ok = product_req("  ชา  ", "เครื่องดื่ม", 3_500);
        assert_eq!(product_input(ok).ok().unwrap().name, "ชา");
        for price in [0, -5, PRICE_MAX_SATANG + 1] {
            assert!(product_input(product_req("ชา", "", price)).is_err());
        }
        assert!(product_input(product_req("   ", "", 100)).is_err());
    }

    #[test]
    fn groups_have_no_price_and_colours_must_be_hex() {
        let group = ProductReq {
            is_group: true,
            ..product_req("ชาไทย", "", 999)
        };
        assert_eq!(product_input(group).ok().unwrap().price_satang, 0);
        let colored = ProductReq {
            color: Some(" #e5484d ".into()),
            ..product_req("ชา", "", 100)
        };
        assert_eq!(
            product_input(colored).ok().unwrap().color.as_deref(),
            Some("#E5484D")
        );
        let wrong = ProductReq {
            color: Some("red".into()),
            ..product_req("ชา", "", 100)
        };
        assert!(product_input(wrong).is_err());
    }

    #[test]
    fn sniffs_only_known_images() {
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(
            sniff_image(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]),
            Some("image/png")
        );
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"<svg xmlns=...>"), None);
        assert_eq!(sniff_image(b"GIF89a"), None);
    }
}
