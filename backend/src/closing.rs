//! Closing the day: the takings of one day frozen into a report (sales by cash and QR, payments the
//! bank check confirmed versus ones confirmed by hand, cancellations after payment) with the cash
//! counted in the drawer and the difference. It replaces "delete everything" as the way to end a day.

use axum::{
    Json,
    extract::{Query, State},
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    audit::{self, action},
    auth::Admin,
    dashboard::{self, Range},
    error::{ApiError, ApiResult},
    state::AppState,
};

const NOTE_MAX_CHARS: usize = 200;
const COUNTED_MAX_SATANG: i64 = 100_000_000;
const LIST_LIMIT: i64 = 60;
const BANGKOK_OFFSET_HOURS: i64 = 7;

fn today() -> String {
    (Utc::now() + chrono::Duration::hours(BANGKOK_OFFSET_HOURS))
        .format("%Y-%m-%d")
        .to_string()
}

fn revenue_of(report: &Value, method: &str) -> i64 {
    report["methods"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["method"] == method))
        .and_then(|r| r["revenue"].as_i64())
        .unwrap_or(0)
}

/// Everything a closing report shows for one day, worked out from the live records.
pub async fn report(state: &AppState, day: &str) -> ApiResult<Value> {
    let range = Range::parse("day", day).ok_or(ApiError::BadRequest("วันที่ไม่ถูกต้อง"))?;
    if day > today().as_str() {
        return Err(ApiError::BadRequest("ปิดยอดของวันที่ยังมาไม่ถึงไม่ได้"));
    }
    let built =
        dashboard::build(&state.pool, state.shop.id, &range, Utc::now().naive_utc()).await?;
    let (from, to) = range.utc_bounds();
    let trail = audit::for_period(&state.pool, state.shop.id, &from, &to).await?;
    let (verified, verified_sum): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(amount_satang), 0) FROM payment_slips \
         WHERE shop_id = ? AND method = 'promptpay' AND verified = 1 AND paid_at >= ? AND paid_at < ?",
    )
    .bind(state.shop.id)
    .bind(&from)
    .bind(&to)
    .fetch_one(&state.pool)
    .await?;
    let closed: Option<(String, String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT closed_at, closed_by, cash_counted_satang, difference_satang FROM day_closes \
         WHERE shop_id = ? AND day = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(state.shop.id)
    .bind(day)
    .fetch_optional(&state.pool)
    .await?;
    Ok(json!({
        "day": day,
        "orders": built["totals"]["orders"],
        "revenue_satang": built["totals"]["revenue"],
        "cash_satang": revenue_of(&built, "cash"),
        "qr_satang": revenue_of(&built, "promptpay"),
        "items_sold": built["totals"]["items"],
        "discount_satang": built["totals"]["discounts"],
        "top_items": built["top_items"],
        "manual_confirms": trail["counts"]["confirm_manual"].as_i64().unwrap_or(0),
        "verified_payments": verified,
        "verified_satang": verified_sum,
        "cancelled_paid": built["cancelled"]["after_payment"],
        "closed": closed.map(|(at, by, counted, diff)| json!({
            "closed_at": at, "closed_by": by, "cash_counted_satang": counted, "difference_satang": diff
        })),
    }))
}

#[derive(Deserialize)]
pub struct DayQuery {
    day: Option<String>,
}

/// The report of a day as it stands now (the page shows it before the owner presses "close").
pub async fn preview(
    _admin: Admin,
    State(state): State<AppState>,
    Query(query): Query<DayQuery>,
) -> ApiResult<Json<Value>> {
    let day = query.day.unwrap_or_else(today);
    Ok(Json(report(&state, &day).await?))
}

#[derive(Deserialize)]
pub struct CloseReq {
    day: Option<String>,
    /// Cash counted in the drawer, in satang; left out when nobody counted.
    cash_counted_satang: Option<i64>,
    #[serde(default)]
    note: String,
}

pub async fn close(
    Admin(admin): Admin,
    State(state): State<AppState>,
    Json(req): Json<CloseReq>,
) -> ApiResult<Json<Value>> {
    let day = req.day.unwrap_or_else(today);
    let note = crate::admin::clean_text(&req.note, NOTE_MAX_CHARS)
        .ok_or(ApiError::BadRequest("หมายเหตุยาวเกิน 200 ตัวอักษร"))?;
    if req
        .cash_counted_satang
        .is_some_and(|c| !(0..=COUNTED_MAX_SATANG).contains(&c))
    {
        return Err(ApiError::BadRequest("จำนวนเงินสดที่นับไม่ถูกต้อง"));
    }
    let report = report(&state, &day).await?;
    let cash = report["cash_satang"].as_i64().unwrap_or(0);
    let difference = req.cash_counted_satang.map(|counted| counted - cash);
    sqlx::query(
        "INSERT INTO day_closes (shop_id, day, closed_by, orders, revenue_satang, cash_satang, \
         qr_satang, manual_confirms, cancelled_paid, cash_counted_satang, difference_satang, note) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(state.shop.id)
    .bind(&day)
    .bind(&admin.username)
    .bind(report["orders"].as_i64().unwrap_or(0))
    .bind(report["revenue_satang"].as_i64().unwrap_or(0))
    .bind(cash)
    .bind(report["qr_satang"].as_i64().unwrap_or(0))
    .bind(report["manual_confirms"].as_i64().unwrap_or(0))
    .bind(report["cancelled_paid"]["orders"].as_i64().unwrap_or(0))
    .bind(req.cash_counted_satang)
    .bind(difference)
    .bind(&note)
    .execute(&state.pool)
    .await?;
    let detail = match difference {
        Some(d) => format!("ปิดยอด {day} เงินสดต่าง {d} สตางค์"),
        None => format!("ปิดยอด {day}"),
    };
    audit::record(&state, &admin, action::CLOSE_DAY, None, &detail).await;
    Ok(Json(
        json!({ "ok": true, "difference_satang": difference, "report": report }),
    ))
}

type CloseRow = (
    i64,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    String,
);

/// Past closings, newest first.
pub async fn list(_admin: Admin, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let rows: Vec<CloseRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, day, strftime('%Y-%m-%d %H:%M', closed_at, '+{BANGKOK_OFFSET_HOURS} hours'), closed_by, orders, \
         revenue_satang, cash_satang, qr_satang, manual_confirms, cancelled_paid, cash_counted_satang, \
         difference_satang, note FROM day_closes WHERE shop_id = ? ORDER BY id DESC LIMIT {LIST_LIMIT}"
    )))
    .bind(state.shop.id)
    .fetch_all(&state.pool)
    .await?;
    let closes: Vec<Value> = rows
        .into_iter()
        .map(
            |(
                id,
                day,
                at,
                by,
                orders,
                revenue,
                cash,
                qr,
                manual,
                cancelled,
                counted,
                diff,
                note,
            )| {
                json!({ "id": id, "day": day, "closed_at": at, "closed_by": by, "orders": orders,
                    "revenue_satang": revenue, "cash_satang": cash, "qr_satang": qr,
                    "manual_confirms": manual, "cancelled_paid": cancelled,
                    "cash_counted_satang": counted, "difference_satang": diff, "note": note })
            },
        )
        .collect();
    Ok(Json(json!(closes)))
}
