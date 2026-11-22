use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::AssertSqlSafe;

use crate::{
    auth::Authed,
    error::{ApiError, ApiResult},
    events::Event,
    payment::{PingStatus, Route},
    settings::{self, PaySettings, digits_of},
    state::{AppState, PayRuntime},
};

pub const IMAGE_MAX_BYTES: usize = 1_048_576;
const NAME_MAX_CHARS: usize = 60;
const CATEGORY_MAX_CHARS: usize = 30;
const PRICE_MAX_SATANG: i64 = 10_000_000;
const FIELD_MAX_CHARS: usize = 200;
const BULK_MAX_IDS: usize = 500;

// ---------------------------------------------------------------- products

#[derive(Deserialize)]
pub struct ProductReq {
    name: String,
    category: String,
    price_satang: i64,
}

struct ProductInput {
    name: String,
    category: String,
    price_satang: i64,
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
    if !(1..=PRICE_MAX_SATANG).contains(&req.price_satang) {
        return Err(ApiError::BadRequest("ราคาต้องมากกว่า 0 และไม่เกิน 100,000 บาท"));
    }
    Ok(ProductInput {
        name,
        category,
        price_satang: req.price_satang,
    })
}

fn products_changed(state: &AppState) {
    state.publish(Event::new("products", json!({})));
}

pub async fn create_product(
    _auth: Authed,
    State(state): State<AppState>,
    Json(req): Json<ProductReq>,
) -> ApiResult<Json<Value>> {
    let input = product_input(req)?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO products (shop_id, name, category, price_satang, sort) \
         VALUES (?, ?, ?, ?, (SELECT COALESCE(MAX(sort), -1) + 1 FROM products WHERE shop_id = ?)) \
         RETURNING id",
    )
    .bind(state.shop.id)
    .bind(&input.name)
    .bind(&input.category)
    .bind(input.price_satang)
    .bind(state.shop.id)
    .fetch_one(&state.pool)
    .await?;
    products_changed(&state);
    Ok(Json(json!({ "id": id })))
}

pub async fn update_product(
    _auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ProductReq>,
) -> ApiResult<Json<Value>> {
    let input = product_input(req)?;
    let updated = sqlx::query(
        "UPDATE products SET name = ?, category = ?, price_satang = ? \
         WHERE id = ? AND shop_id = ? AND active = 1",
    )
    .bind(&input.name)
    .bind(&input.category)
    .bind(input.price_satang)
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

/// Soft delete: past orders keep their own copy of name and price.
pub async fn delete_product(
    _auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let updated = sqlx::query(
        "UPDATE products SET active = 0, image_mime = NULL, image_data = NULL, \
         image_v = image_v + 1 WHERE id = ? AND shop_id = ? AND active = 1",
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

const SOFT_DELETE_SQL: &str = "UPDATE products SET active = 0, image_mime = NULL, \
     image_data = NULL, image_v = image_v + 1 WHERE shop_id = ? AND active = 1";

#[derive(Deserialize)]
pub struct BulkDeleteReq {
    ids: Vec<i64>,
}

/// Deletes the selected products; unknown or already-deleted ids are ignored.
pub async fn bulk_delete_products(
    _auth: Authed,
    State(state): State<AppState>,
    Json(req): Json<BulkDeleteReq>,
) -> ApiResult<Json<Value>> {
    if req.ids.is_empty() || req.ids.len() > BULK_MAX_IDS {
        return Err(ApiError::BadRequest("เลือกสินค้าที่จะลบก่อน"));
    }
    let mut tx = state.pool.begin().await?;
    let mut deleted = 0;
    for id in &req.ids {
        let done = sqlx::query(AssertSqlSafe(format!("{SOFT_DELETE_SQL} AND id = ?")))
            .bind(state.shop.id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        deleted += done.rows_affected();
    }
    tx.commit().await?;
    products_changed(&state);
    Ok(Json(json!({ "deleted": deleted })))
}

pub async fn delete_all_products(
    _auth: Authed,
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
    _auth: Authed,
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
    _auth: Authed,
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

pub async fn get_settings(_auth: Authed, State(state): State<AppState>) -> Json<Value> {
    Json(settings_view(&state.pay()))
}

pub async fn put_settings(
    _auth: Authed,
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
pub async fn test_settings(_auth: Authed, State(state): State<AppState>) -> ApiResult<Json<Value>> {
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

    #[test]
    fn product_validation() {
        let ok = ProductReq {
            name: "  ชา  ".into(),
            category: "เครื่องดื่ม".into(),
            price_satang: 3_500,
        };
        assert_eq!(product_input(ok).ok().unwrap().name, "ชา");
        for price in [0, -5, PRICE_MAX_SATANG + 1] {
            let bad = ProductReq {
                name: "ชา".into(),
                category: String::new(),
                price_satang: price,
            };
            assert!(product_input(bad).is_err());
        }
        let blank = ProductReq {
            name: "   ".into(),
            category: String::new(),
            price_satang: 100,
        };
        assert!(product_input(blank).is_err());
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
