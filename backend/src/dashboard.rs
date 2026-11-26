//! Sales summary per day / month / year, and a CSV export of the same range.
//!
//! A "sale" is an order that was paid and not cancelled (`paid`, `called`, `done`).
//! Dates are Bangkok local dates; the database stores UTC, so ranges are converted to UTC
//! bounds (which also lets the `paid_at` index be used).

use anyhow::Result;
use axum::{
    Json,
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    auth::Admin,
    error::{ApiError, ApiResult},
    payment::format_baht,
    state::AppState,
};

const BANGKOK_OFFSET_HOURS: i64 = 7;
const SOLD: &str = "('paid', 'called', 'done')";
const TOP_ITEMS: i64 = 10;
const MIN_YEAR: i32 = 2000;
const MAX_YEAR: i32 = 2200;
const THAI_MONTHS_SHORT: [&str; 12] = [
    "ม.ค.",
    "ก.พ.",
    "มี.ค.",
    "เม.ย.",
    "พ.ค.",
    "มิ.ย.",
    "ก.ค.",
    "ส.ค.",
    "ก.ย.",
    "ต.ค.",
    "พ.ย.",
    "ธ.ค.",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Day,
    Month,
    Year,
}

/// Half-open range `[start, end)` of Bangkok local dates.
#[derive(Clone, Debug)]
pub struct Range {
    pub period: Period,
    pub start: NaiveDate,
    pub end: NaiveDate,
}

fn first_of_month(year: i32, month: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, 1)
}

fn add_months(date: NaiveDate, months: i32) -> Option<NaiveDate> {
    let index = date.year() * 12 + date.month0() as i32 + months;
    first_of_month(index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
}

impl Range {
    /// `day`: `2026-11-22`, `month`: `2026-11`, `year`: `2026`.
    pub fn parse(period: &str, date: &str) -> Option<Self> {
        let (period, start) = match period {
            "day" => (
                Period::Day,
                NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?,
            ),
            "month" => (
                Period::Month,
                NaiveDate::parse_from_str(&format!("{date}-01"), "%Y-%m-%d").ok()?,
            ),
            "year" => (
                Period::Year,
                NaiveDate::parse_from_str(&format!("{date}-01-01"), "%Y-%m-%d").ok()?,
            ),
            _ => return None,
        };
        if !(MIN_YEAR..=MAX_YEAR).contains(&start.year()) {
            return None;
        }
        Self::starting(period, start)
    }

    fn starting(period: Period, start: NaiveDate) -> Option<Self> {
        let end = match period {
            Period::Day => start.checked_add_signed(Duration::days(1))?,
            Period::Month => add_months(start, 1)?,
            Period::Year => add_months(start, 12)?,
        };
        Some(Self { period, start, end })
    }

    pub fn previous(&self) -> Option<Self> {
        let start = match self.period {
            Period::Day => self.start.checked_sub_signed(Duration::days(1))?,
            Period::Month => add_months(self.start, -1)?,
            Period::Year => add_months(self.start, -12)?,
        };
        Self::starting(self.period, start)
    }

    fn utc(date: NaiveDate) -> String {
        let local = date.and_hms_opt(0, 0, 0).unwrap_or_default();
        (local - Duration::hours(BANGKOK_OFFSET_HOURS))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    /// `[from, to)` as UTC timestamps in the database's text format.
    pub fn utc_bounds(&self) -> (String, String) {
        (Self::utc(self.start), Self::utc(self.end))
    }

    fn utc_dt(date: NaiveDate) -> NaiveDateTime {
        date.and_hms_opt(0, 0, 0).unwrap_or_default() - Duration::hours(BANGKOK_OFFSET_HOURS)
    }

    /// `[from, to)` as UTC datetimes.
    fn utc_bounds_dt(&self) -> (NaiveDateTime, NaiveDateTime) {
        (Self::utc_dt(self.start), Self::utc_dt(self.end))
    }

    /// strftime pattern (applied to Bangkok time) that picks the chart bucket of a sale.
    fn bucket_format(&self) -> &'static str {
        match self.period {
            Period::Day => "%H",
            Period::Month => "%d",
            Period::Year => "%m",
        }
    }

    /// Every bucket of the chart, in order, as `(key, label)`.
    pub fn buckets(&self) -> Vec<(String, String)> {
        match self.period {
            Period::Day => (0..24)
                .map(|h| (format!("{h:02}"), format!("{h:02}:00")))
                .collect(),
            Period::Month => (1..=(self.end - self.start).num_days())
                .map(|d| (format!("{d:02}"), d.to_string()))
                .collect(),
            Period::Year => THAI_MONTHS_SHORT
                .iter()
                .enumerate()
                .map(|(i, name)| (format!("{:02}", i + 1), (*name).to_string()))
                .collect(),
        }
    }

    pub fn file_label(&self) -> String {
        match self.period {
            Period::Day => self.start.format("%Y-%m-%d").to_string(),
            Period::Month => self.start.format("%Y-%m").to_string(),
            Period::Year => self.start.format("%Y").to_string(),
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Totals {
    orders: i64,
    revenue: i64,
}

async fn totals(pool: &SqlitePool, shop_id: i64, from: &str, to: &str) -> Result<Totals> {
    let sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(total_satang), 0) FROM orders \
         WHERE shop_id = ? AND status IN {SOLD} AND paid_at >= ? AND paid_at < ?"
    );
    let (orders, revenue): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(shop_id)
        .bind(from)
        .bind(to)
        .fetch_one(pool)
        .await?;
    Ok(Totals { orders, revenue })
}

async fn grouped(
    pool: &SqlitePool,
    shop_id: i64,
    range: &Range,
    column: &str,
) -> Result<Vec<(String, i64, i64)>> {
    let (from, to) = range.utc_bounds();
    let sql = format!(
        "SELECT {column}, COUNT(*), COALESCE(SUM(total_satang), 0) FROM orders \
         WHERE shop_id = ? AND status IN {SOLD} AND paid_at >= ? AND paid_at < ? \
         GROUP BY {column} ORDER BY 3 DESC"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(shop_id)
        .bind(&from)
        .bind(&to)
        .fetch_all(pool)
        .await?)
}

fn pct_change(now: i64, before: i64) -> Option<f64> {
    (before > 0).then(|| (now - before) as f64 * 100.0 / before as f64)
}

fn rows_json(rows: Vec<(String, i64, i64)>, key: &str) -> Vec<Value> {
    rows.into_iter()
        .map(|(name, orders, revenue)| json!({ key: name, "orders": orders, "revenue": revenue }))
        .collect()
}

fn db_time(at: NaiveDateTime) -> String {
    at.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `now` is the current UTC time. While the range is still running (today, this month, this
/// year) it is compared with the same elapsed part of the previous range, not the whole of it.
pub async fn build(
    pool: &SqlitePool,
    shop_id: i64,
    range: &Range,
    now_utc: NaiveDateTime,
) -> Result<Value> {
    let (from, to) = range.utc_bounds();
    let now = totals(pool, shop_id, &from, &to).await?;
    let (range_from, range_to) = range.utc_bounds_dt();
    let still_running = range_from <= now_utc && now_utc < range_to;
    let before = match range.previous() {
        Some(prev) => {
            let (prev_from, prev_to) = prev.utc_bounds_dt();
            let prev_to = if still_running {
                (prev_from + (now_utc - range_from)).min(prev_to)
            } else {
                prev_to
            };
            totals(pool, shop_id, &db_time(prev_from), &db_time(prev_to)).await?
        }
        None => Totals::default(),
    };

    let methods = grouped(pool, shop_id, range, "method").await?;
    let sources = grouped(pool, shop_id, range, "source").await?;

    let bank_sql = format!(
        "SELECT COALESCE(p.bank, 'manual'), COUNT(*), COALESCE(SUM(o.total_satang), 0) \
         FROM orders o LEFT JOIN payments p ON p.order_id = o.id \
         WHERE o.shop_id = ? AND o.method = 'promptpay' AND o.status IN {SOLD} \
         AND o.paid_at >= ? AND o.paid_at < ? GROUP BY 1 ORDER BY 3 DESC"
    );
    let banks: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(bank_sql))
        .bind(shop_id)
        .bind(&from)
        .bind(&to)
        .fetch_all(pool)
        .await?;

    let trend_sql = format!(
        "SELECT strftime('{}', paid_at, '+{BANGKOK_OFFSET_HOURS} hours'), method, COUNT(*), \
         COALESCE(SUM(total_satang), 0) FROM orders \
         WHERE shop_id = ? AND status IN {SOLD} AND paid_at >= ? AND paid_at < ? GROUP BY 1, 2",
        range.bucket_format()
    );
    let trend_rows: Vec<(String, String, i64, i64)> =
        sqlx::query_as(sqlx::AssertSqlSafe(trend_sql))
            .bind(shop_id)
            .bind(&from)
            .bind(&to)
            .fetch_all(pool)
            .await?;
    let trend: Vec<Value> = range
        .buckets()
        .into_iter()
        .map(|(key, label)| {
            let pick = |method: &str| {
                trend_rows
                    .iter()
                    .find(|(k, m, _, _)| *k == key && m == method)
                    .map_or((0, 0), |(_, _, orders, revenue)| (*orders, *revenue))
            };
            let (qr_orders, qr_revenue) = pick("promptpay");
            let (cash_orders, cash_revenue) = pick("cash");
            json!({
                "key": key, "label": label,
                "qr_orders": qr_orders, "qr_revenue": qr_revenue,
                "cash_orders": cash_orders, "cash_revenue": cash_revenue,
            })
        })
        .collect();
    let peak = trend
        .iter()
        .filter_map(|b| {
            let revenue = b["qr_revenue"].as_i64()? + b["cash_revenue"].as_i64()?;
            (revenue > 0).then(|| (revenue, b["label"].clone()))
        })
        .max_by_key(|(revenue, _)| *revenue)
        .map(|(revenue, label)| json!({ "label": label, "revenue": revenue }));

    let items_sql = format!(
        "SELECT oi.name, SUM(oi.qty), COALESCE(SUM(oi.qty * oi.unit_price_satang), 0) \
         FROM order_items oi JOIN orders o ON o.id = oi.order_id \
         WHERE o.shop_id = ? AND o.status IN {SOLD} AND o.paid_at >= ? AND o.paid_at < ? \
         GROUP BY oi.name ORDER BY 2 DESC, 3 DESC"
    );
    let items: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(items_sql))
        .bind(shop_id)
        .bind(&from)
        .bind(&to)
        .fetch_all(pool)
        .await?;
    let items_sold: i64 = items.iter().map(|(_, qty, _)| qty).sum();
    let top_items: Vec<Value> = items
        .into_iter()
        .take(TOP_ITEMS as usize)
        .map(|(name, qty, revenue)| json!({ "name": name, "qty": qty, "revenue": revenue }))
        .collect();

    let (cancelled_after, cancelled_after_sum): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(total_satang), 0) FROM orders \
         WHERE shop_id = ? AND status = 'cancelled' AND paid_at IS NOT NULL \
         AND paid_at >= ? AND paid_at < ?",
    )
    .bind(shop_id)
    .bind(&from)
    .bind(&to)
    .fetch_one(pool)
    .await?;
    let (cancelled_unpaid, cancelled_unpaid_sum): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(total_satang), 0) FROM orders \
         WHERE shop_id = ? AND status = 'cancelled' AND paid_at IS NULL \
         AND created_at >= ? AND created_at < ?",
    )
    .bind(shop_id)
    .bind(&from)
    .bind(&to)
    .fetch_one(pool)
    .await?;

    let sold_discounts_sql = format!(
        "SELECT COALESCE(SUM(discount_satang), 0) FROM orders          WHERE shop_id = ? AND status IN {SOLD} AND paid_at >= ? AND paid_at < ?"
    );
    let discounts: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sold_discounts_sql))
        .bind(shop_id)
        .bind(&from)
        .bind(&to)
        .fetch_one(pool)
        .await?;
    let reasons: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT cancel_reason, COUNT(*), COALESCE(SUM(total_satang), 0) FROM orders          WHERE shop_id = ? AND status = 'cancelled' AND cancel_reason <> ''          AND COALESCE(paid_at, created_at) >= ? AND COALESCE(paid_at, created_at) < ?          GROUP BY cancel_reason ORDER BY COUNT(*) DESC, cancel_reason LIMIT 10",
    )
    .bind(shop_id)
    .bind(&from)
    .bind(&to)
    .fetch_all(pool)
    .await?;
    let reasons: Vec<Value> = reasons
        .into_iter()
        .map(|(reason, orders, revenue)| json!({ "reason": reason, "orders": orders, "revenue": revenue }))
        .collect();

    let average = if now.orders > 0 {
        now.revenue / now.orders
    } else {
        0
    };
    Ok(json!({
        "period": match range.period { Period::Day => "day", Period::Month => "month", Period::Year => "year" },
        "from": range.start.to_string(),
        "to": (range.end - Duration::days(1)).to_string(),
        "totals": { "orders": now.orders, "revenue": now.revenue, "average": average, "items": items_sold, "discounts": discounts },
        "previous": {
            "orders": before.orders,
            "revenue": before.revenue,
            "same_period": still_running,
            "revenue_change_pct": pct_change(now.revenue, before.revenue),
            "orders_change_pct": pct_change(now.orders, before.orders),
        },
        "methods": rows_json(methods, "method"),
        "sources": rows_json(sources, "source"),
        "banks": rows_json(banks, "bank"),
        "trend": trend,
        "peak": peak,
        "top_items": top_items,
        "cancelled": {
            "after_payment": { "orders": cancelled_after, "revenue": cancelled_after_sum },
            "unpaid": { "orders": cancelled_unpaid, "revenue": cancelled_unpaid_sum },
            "reasons": reasons,
        },
    }))
}

/// A spreadsheet would run a cell that starts with these as a formula.
fn csv_cell(value: &str) -> String {
    let safe = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    if safe.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

type SaleRow = (
    i64,
    Option<i64>,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
);

pub async fn csv(
    pool: &SqlitePool,
    shop_id: i64,
    range: &Range,
    bill: &crate::bill::BillSettings,
) -> Result<String> {
    let (from, to) = range.utc_bounds();
    let sql = format!(
        "SELECT o.id, o.queue_no, COALESCE(strftime('%Y-%m-%d %H:%M', o.paid_at, '+{BANGKOK_OFFSET_HOURS} hours'), ''), \
         o.method, o.source, o.total_satang, o.status, COALESCE(p.bank, ''), \
         COALESCE((SELECT group_concat(name || ' x' || qty, '; ') FROM order_items WHERE order_id = o.id), '') \
         FROM orders o LEFT JOIN payments p ON p.order_id = o.id \
         WHERE o.shop_id = ? AND o.paid_at >= ? AND o.paid_at < ? ORDER BY o.paid_at, o.id"
    );
    let rows: Vec<SaleRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(shop_id)
        .bind(&from)
        .bind(&to)
        .fetch_all(pool)
        .await?;
    let mut out =
        String::from("\u{FEFF}ออเดอร์,คิว,เวลาชำระ,ช่องทาง,ที่มา,ยอด(บาท),สถานะ,ธนาคาร,รายการ\n");
    for (id, queue, paid, method, source, total, status, bank, items) in rows {
        let method = if method == "cash" {
            "เงินสด"
        } else {
            "QR พร้อมเพย์"
        };
        let source = if source == "self" {
            "ลูกค้าสั่งเอง"
        } else {
            "หน้าร้าน"
        };
        let status = match status.as_str() {
            "cancelled" => "ยกเลิก",
            "done" => "รับของแล้ว",
            _ => "ชำระแล้ว",
        };
        let queue = queue.map(|no| bill.queue_label(no)).unwrap_or_default();
        let cells = [
            id.to_string(),
            queue,
            paid,
            method.to_string(),
            source.to_string(),
            format_baht(total),
            status.to_string(),
            bank.to_uppercase(),
            items,
        ];
        out.push_str(
            &cells
                .iter()
                .map(|c| csv_cell(c))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct Params {
    period: String,
    date: String,
}

fn range_of(params: &Params) -> ApiResult<Range> {
    Range::parse(&params.period, &params.date).ok_or(ApiError::BadRequest("ช่วงเวลาไม่ถูกต้อง"))
}

/// Slips shown per page. The page is cut here, not in the browser: a busy month holds thousands,
/// and sending them all made the dashboard crawl on a weak connection.
const SLIPS_PER_PAGE: i64 = 10;
/// Furthest page a request may ask for (a guard against absurd offsets).
const SLIPS_MAX_PAGE: i64 = 100_000;

type SlipRow = (
    i64,
    Option<i64>,
    i64,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    Option<i64>,
);

#[derive(Deserialize)]
pub struct SlipsParams {
    period: String,
    date: String,
    /// "cash", "promptpay", or empty for both.
    #[serde(default)]
    method: String,
    #[serde(default)]
    page: i64,
}

fn slip_json(row: SlipRow, bill: &crate::bill::BillSettings) -> Value {
    let (order_id, queue_no, amount, verified, payer, bank, reference, items, at, method, received) =
        row;
    json!({
        "order_id": order_id,
        "queue_label": queue_no.map(|no| bill.queue_label(no)),
        "amount_satang": amount,
        "verified": verified == 1,
        "payer_name": payer,
        "bank": bank,
        "reference": reference,
        "items": serde_json::from_str::<Value>(&items).unwrap_or_else(|_| json!([])),
        "paid_at": at,
        "method": method,
        "cash_received_satang": received,
    })
}

/// One page of the period's payments (cash and PromptPay), newest first: the slip (payer, bank,
/// reference, what was bought), with the counts and sums the filter chips show. Kept in
/// `payment_slips`, so past days stay available.
async fn slips(
    pool: &SqlitePool,
    shop_id: i64,
    range: &Range,
    bill: &crate::bill::BillSettings,
    method: &str,
    page: i64,
) -> Result<Value> {
    let (from, to) = range.utc_bounds();
    let sums: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT method, COUNT(*), COALESCE(SUM(amount_satang), 0) FROM payment_slips \
         WHERE shop_id = ? AND method IN ('promptpay', 'cash') AND paid_at >= ? AND paid_at < ? \
         GROUP BY method",
    )
    .bind(shop_id)
    .bind(&from)
    .bind(&to)
    .fetch_all(pool)
    .await?;
    let of = |name: &str| {
        sums.iter()
            .find(|(m, _, _)| m == name)
            .map_or((0, 0), |(_, n, sum)| (*n, *sum))
    };
    let (cash, promptpay) = (of("cash"), of("promptpay"));
    let (total, sum) = match method {
        "cash" => cash,
        "promptpay" => promptpay,
        _ => (cash.0 + promptpay.0, cash.1 + promptpay.1),
    };
    let rows: Vec<SlipRow> = sqlx::query_as(
        "SELECT order_id, queue_no, amount_satang, verified, payer_name, bank, external_id, items, \
         strftime('%Y-%m-%d %H:%M:%S', paid_at, '+' || ? || ' hours'), method, cash_received_satang \
         FROM payment_slips WHERE shop_id = ? AND method IN ('promptpay', 'cash') \
         AND (? = '' OR method = ?) AND paid_at >= ? AND paid_at < ? \
         ORDER BY paid_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(BANGKOK_OFFSET_HOURS)
    .bind(shop_id)
    .bind(method)
    .bind(method)
    .bind(&from)
    .bind(&to)
    .bind(SLIPS_PER_PAGE)
    .bind(page * SLIPS_PER_PAGE)
    .fetch_all(pool)
    .await?;
    Ok(json!({
        "rows": rows.into_iter().map(|row| slip_json(row, bill)).collect::<Vec<_>>(),
        "page": page,
        "per_page": SLIPS_PER_PAGE,
        "total": total,
        "sum_satang": sum,
        "counts": { "all": cash.0 + promptpay.0, "cash": cash.0, "promptpay": promptpay.0 },
    }))
}

/// The slips card of the dashboard, one page at a time.
pub async fn slips_page(
    _admin: Admin,
    State(state): State<AppState>,
    Query(params): Query<SlipsParams>,
) -> ApiResult<Json<Value>> {
    let range =
        Range::parse(&params.period, &params.date).ok_or(ApiError::BadRequest("ช่วงเวลาไม่ถูกต้อง"))?;
    let method = match params.method.as_str() {
        "" | "all" => "",
        "cash" => "cash",
        "promptpay" => "promptpay",
        _ => return Err(ApiError::BadRequest("วิธีรับเงินไม่ถูกต้อง")),
    };
    let page = params.page.clamp(0, SLIPS_MAX_PAGE);
    Ok(Json(
        slips(
            &state.pool,
            state.shop.id,
            &range,
            &state.bill(),
            method,
            page,
        )
        .await?,
    ))
}

pub async fn data(
    _admin: Admin,
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Json<Value>> {
    let range = range_of(&params)?;
    let now = chrono::Utc::now().naive_utc();
    let mut body = build(&state.pool, state.shop.id, &range, now).await?;
    let (from, to) = range.utc_bounds();
    body["audit"] = crate::audit::for_period(&state.pool, state.shop.id, &from, &to).await?;
    Ok(Json(body))
}

pub async fn export(
    _admin: Admin,
    State(state): State<AppState>,
    Query(params): Query<Params>,
) -> ApiResult<Response> {
    let range = range_of(&params)?;
    let body = csv(&state.pool, state.shop.id, &range, &state.bill()).await?;
    let disposition = format!("attachment; filename=\"sales-{}.csv\"", range.file_label());
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn range(period: &str, date: &str) -> Range {
        Range::parse(period, date).unwrap()
    }

    fn at(utc: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(utc, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn parses_and_rejects_ranges() {
        assert!(Range::parse("day", "2026-11-22").is_some());
        assert!(Range::parse("month", "2026-11").is_some());
        assert!(Range::parse("year", "2026").is_some());
        assert!(Range::parse("week", "2026-11-22").is_none());
        assert!(Range::parse("day", "2026-13-01").is_none());
        assert!(Range::parse("day", "garbage").is_none());
        assert!(Range::parse("year", "1850").is_none());
    }

    #[test]
    fn ranges_cover_whole_units_and_step_back() {
        let day = range("day", "2026-12-31");
        assert_eq!(day.end.to_string(), "2027-01-01");
        let month = range("month", "2026-12");
        assert_eq!(month.end.to_string(), "2027-01-01");
        assert_eq!(month.previous().unwrap().start.to_string(), "2026-11-01");
        let jan = range("month", "2026-01");
        assert_eq!(jan.previous().unwrap().start.to_string(), "2025-12-01");
        let leap = range("month", "2024-02");
        assert_eq!(leap.buckets().len(), 29);
        assert_eq!(range("month", "2026-02").buckets().len(), 28);
        assert_eq!(
            range("year", "2026").previous().unwrap().start.to_string(),
            "2025-01-01"
        );
        assert_eq!(range("day", "2026-11-22").buckets().len(), 24);
        assert_eq!(range("year", "2026").buckets().len(), 12);
    }

    #[test]
    fn bangkok_midnight_is_17h_utc_the_day_before() {
        let (from, to) = range("day", "2026-11-22").utc_bounds();
        assert_eq!(from, "2026-11-21 17:00:00");
        assert_eq!(to, "2026-11-22 17:00:00");
    }

    #[test]
    fn csv_cells_are_escaped_against_formula_injection() {
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_cell("=1+1"), "'=1+1");
        assert_eq!(csv_cell("@x"), "'@x");
        assert_eq!(csv_cell("plain"), "plain");
    }

    #[test]
    fn change_is_undefined_without_a_baseline() {
        assert_eq!(pct_change(100, 0), None);
        assert_eq!(pct_change(150, 100), Some(50.0));
        assert_eq!(pct_change(50, 100), Some(-50.0));
    }

    #[tokio::test]
    async fn a_running_month_is_compared_with_the_same_days_of_the_last_one() {
        let path = std::env::temp_dir().join(format!("posmini-dash2-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let pool = db::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
        sqlx::query("INSERT INTO shops (id, slug, name) VALUES (1, 's', 'S')")
            .execute(&pool)
            .await
            .unwrap();
        // (paid_at UTC, satang): 5 Oct and 28 Oct (local), then 10 Nov (local).
        for (id, paid_at, total) in [
            (1, "2026-10-05 03:00:00", 3_000),
            (2, "2026-10-28 03:00:00", 5_000),
            (3, "2026-11-10 03:00:00", 4_000),
        ] {
            sqlx::query(
                "INSERT INTO orders (id, shop_id, token, queue_date, status, method, source, total_satang, paid_at, created_at)                  VALUES (?, 1, ?, '2026-11-10', 'done', 'cash', 'pos', ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("t{id}"))
            .bind(total)
            .bind(paid_at)
            .bind(paid_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        let november = range("month", "2026-11");
        // 22 Nov 15:00 local: only 1-22 Oct counts, so the 28 Oct sale is left out.
        let running = build(&pool, 1, &november, at("2026-11-22 08:00:00"))
            .await
            .unwrap();
        assert_eq!(running["previous"]["same_period"], true);
        assert_eq!(running["previous"]["revenue"], 3_000);
        // once November is over the whole of October is the baseline
        let finished = build(&pool, 1, &november, at("2026-12-05 00:00:00"))
            .await
            .unwrap();
        assert_eq!(finished["previous"]["same_period"], false);
        assert_eq!(finished["previous"]["revenue"], 8_000);
        pool.close().await;
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn summarizes_sales_from_the_database() {
        let path = std::env::temp_dir().join(format!("posmini-dash-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let pool = db::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
        sqlx::query("INSERT INTO shops (id, slug, name) VALUES (1, 's', 'S')")
            .execute(&pool)
            .await
            .unwrap();
        // (id, status, method, source, total, paid_at UTC). 22 Nov Bangkok = 21 Nov 17:00 .. 22 Nov 17:00 UTC.
        let rows = [
            (1, "done", "cash", "pos", 4_000, Some("2026-11-22 02:10:00")), // 09:10 local
            (
                2,
                "paid",
                "promptpay",
                "pos",
                5_500,
                Some("2026-11-22 02:40:00"),
            ), // 09:40 local
            (
                3,
                "called",
                "promptpay",
                "self",
                1_000,
                Some("2026-11-22 08:00:00"),
            ), // 15:00 local
            (
                4,
                "cancelled",
                "cash",
                "pos",
                9_900,
                Some("2026-11-22 03:00:00"),
            ), // cancelled after paying
            (5, "cancelled", "cash", "pos", 700, None),
            (6, "done", "cash", "pos", 2_000, Some("2026-11-21 16:59:00")), // 23:59 local on the 21st
            (7, "pending_payment", "promptpay", "pos", 300, None),
        ];
        for (id, status, method, source, total, paid_at) in rows {
            sqlx::query(
                "INSERT INTO orders (id, shop_id, token, queue_date, status, method, source, total_satang, paid_at, created_at) \
                 VALUES (?, 1, ?, '2026-11-22', ?, ?, ?, ?, ?, '2026-11-22 01:00:00')",
            )
            .bind(id)
            .bind(format!("tok{id}"))
            .bind(status)
            .bind(method)
            .bind(source)
            .bind(total)
            .bind(paid_at)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO order_items (order_id, product_id, name, unit_price_satang, qty) VALUES (?, 1, 'ลาเต้', ?, 1)")
                .bind(id)
                .bind(total)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO payments (shop_id, order_id, amount_satang, method, external_id, bank) VALUES (1, 2, 5500, 'promptpay', 'e2', 'scb')")
            .execute(&pool)
            .await
            .unwrap();

        let later = at("2030-01-01 00:00:00");
        let day = build(&pool, 1, &range("day", "2026-11-22"), later)
            .await
            .unwrap();
        assert_eq!(day["totals"]["orders"], 3);
        assert_eq!(day["totals"]["revenue"], 10_500);
        assert_eq!(day["totals"]["average"], 3_500);
        assert_eq!(day["totals"]["items"], 3);
        let method = |m: &str| {
            day["methods"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["method"] == m)
                .cloned()
                .unwrap()
        };
        assert_eq!(method("promptpay")["revenue"], 6_500);
        assert_eq!(method("cash")["orders"], 1);
        assert_eq!(day["banks"][0]["bank"], "scb");
        assert_eq!(day["banks"][1]["bank"], "manual");
        assert_eq!(day["cancelled"]["after_payment"]["orders"], 1);
        assert_eq!(day["cancelled"]["unpaid"]["orders"], 1);
        assert_eq!(day["trend"][9]["cash_revenue"], 4_000);
        assert_eq!(day["trend"][9]["qr_revenue"], 5_500);
        assert_eq!(day["trend"][15]["qr_revenue"], 1_000);
        assert_eq!(day["peak"]["label"], "09:00");
        // the sale at 23:59 on the 21st belongs to the previous day, not this one
        assert_eq!(day["previous"]["revenue"], 2_000);
        assert_eq!(day["previous"]["revenue_change_pct"], 425.0);

        let month = build(&pool, 1, &range("month", "2026-11"), later)
            .await
            .unwrap();
        assert_eq!(month["totals"]["orders"], 4);
        assert_eq!(month["trend"].as_array().unwrap().len(), 30);
        let year = build(&pool, 1, &range("year", "2026"), later)
            .await
            .unwrap();
        assert_eq!(year["totals"]["revenue"], 12_500);
        assert_eq!(year["trend"][10]["cash_revenue"], 6_000);

        let sheet = csv(
            &pool,
            1,
            &range("day", "2026-11-22"),
            &crate::bill::BillSettings::default(),
        )
        .await
        .unwrap();
        assert!(sheet.starts_with('\u{FEFF}'));
        assert_eq!(
            sheet.lines().count(),
            5,
            "header + 3 sales + 1 cancelled after payment"
        );
        assert!(sheet.contains("SCB"));

        pool.close().await;
        let _ = std::fs::remove_file(&path);
    }
}
