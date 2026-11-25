//! The record of who did what: every action that moves money or changes the record leaves one
//! row (never edited or removed) so the owner can see, for example, how many payments a cashier
//! confirmed by hand instead of the bank check.

use anyhow::Result;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{auth::SessionUser, state::AppState};

/// Names stored in `audit_log.action`; the dashboard turns them into Thai.
pub mod action {
    pub const CONFIRM_MANUAL: &str = "confirm_manual";
    pub const CANCEL: &str = "cancel";
    pub const CANCEL_PAID: &str = "cancel_paid";
    pub const CALL_SKIP: &str = "call_skip";
    pub const RESET_SALES: &str = "reset_sales";
    pub const SOLD_OUT: &str = "sold_out";
    pub const DISCOUNT: &str = "discount";
    pub const CLOSE_DAY: &str = "close_day";
    pub const BACKUP: &str = "backup";
    pub const STAFF_ADDED: &str = "staff_added";
    pub const STAFF_CHANGED: &str = "staff_changed";
    pub const STAFF_REMOVED: &str = "staff_removed";
}

const BANGKOK_OFFSET_HOURS: i64 = 7;
const LIST_LIMIT: i64 = 200;
const DETAIL_MAX_CHARS: usize = 300;

/// Writes one row. A failure is logged and swallowed: losing a record must not stop the sale.
pub async fn record(
    state: &AppState,
    who: &SessionUser,
    action: &str,
    order_id: Option<i64>,
    detail: &str,
) {
    record_as(
        state,
        &who.username,
        who.role.as_str(),
        action,
        order_id,
        detail,
    )
    .await;
}

pub async fn record_as(
    state: &AppState,
    actor: &str,
    role: &str,
    action: &str,
    order_id: Option<i64>,
    detail: &str,
) {
    let saved = sqlx::query(
        "INSERT INTO audit_log (shop_id, actor, role, action, order_id, detail) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(state.shop.id)
    .bind(actor)
    .bind(role)
    .bind(action)
    .bind(order_id)
    .bind(detail.chars().take(DETAIL_MAX_CHARS).collect::<String>())
    .execute(&state.pool)
    .await;
    if let Err(err) = saved {
        tracing::error!(error = %err, action, "could not write the audit log");
    }
}

type Row = (String, String, String, String, Option<i64>, String);

/// The rows of a period (newest first, at most 200) and how many there were of each action.
pub async fn for_period(pool: &SqlitePool, shop_id: i64, from: &str, to: &str) -> Result<Value> {
    let sql = format!(
        "SELECT strftime('%Y-%m-%d %H:%M:%S', at, '+{BANGKOK_OFFSET_HOURS} hours'), \
         actor, role, action, order_id, detail \
         FROM audit_log WHERE shop_id = ? AND at >= ? AND at < ? ORDER BY id DESC LIMIT {LIST_LIMIT}"
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(shop_id)
        .bind(from)
        .bind(to)
        .fetch_all(pool)
        .await?;
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT action, COUNT(*) FROM audit_log \
         WHERE shop_id = ? AND at >= ? AND at < ? GROUP BY action",
    )
    .bind(shop_id)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|(at, actor, role, action, order_id, detail)| {
            json!({ "at": at, "actor": actor, "role": role, "action": action,
                    "order_id": order_id, "detail": detail })
        })
        .collect();
    let counts: serde_json::Map<String, Value> =
        counts.into_iter().map(|(a, n)| (a, json!(n))).collect();
    Ok(json!({ "rows": rows, "counts": counts }))
}
