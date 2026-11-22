//! The print queue. Every sheet to print is a row in `print_jobs`, created in the same database
//! transaction that hands out the queue number, and a single worker prints them strictly in
//! that order. So with many orders and one printer: tickets never overtake each other, nothing
//! is lost when the printer is off or the server restarts, and the cashier can see what waits.

use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    events::Event,
    orders,
    printing::{self, Kind, Mode},
    state::AppState,
};

/// How long to wait before trying the same sheet again after a printer error.
const RETRY_SECS: u64 = 5;
/// The worker looks at the queue at least this often even if nobody woke it.
const IDLE_CHECK_SECS: u64 = 30;
/// With this many sheets waiting the pause between sheets is cut short so the line keeps moving.
const BACKLOG_FAST: i64 = 3;
const FAST_GAP_SECS: u64 = 1;
const KEEP_DAYS: i64 = 3;
const LIST_LIMIT: i64 = 30;
/// A browser station keeps a sheet for this long; if it vanishes the sheet is free again.
const LEASE_SECS: i64 = 60;

#[derive(sqlx::FromRow)]
struct Job {
    id: i64,
    order_id: i64,
    kind: String,
    created_at: String,
    attempts: i64,
}

/// Adds sheets to the end of the queue. Inside a transaction when the caller has one open.
pub async fn enqueue(
    conn: &mut SqliteConnection,
    shop_id: i64,
    order_id: i64,
    kinds: &[Kind],
) -> Result<()> {
    for kind in kinds {
        sqlx::query("INSERT INTO print_jobs (shop_id, order_id, kind) VALUES (?, ?, ?)")
            .bind(shop_id)
            .bind(order_id)
            .bind(kind.as_str())
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Tells the worker and every cashier screen that the queue changed.
pub fn changed(state: &AppState) {
    state.print_wake().notify_one();
    let state = state.clone();
    tokio::spawn(async move { publish(&state).await });
}

pub async fn enqueue_kinds(state: &AppState, order_id: i64, kinds: &[Kind]) -> Result<()> {
    if kinds.is_empty() {
        return Ok(());
    }
    let mut conn = state.pool.acquire().await?;
    enqueue(&mut conn, state.shop.id, order_id, kinds).await?;
    drop(conn);
    changed(state);
    Ok(())
}

async fn next_job(pool: &SqlitePool, shop_id: i64) -> Result<Option<Job>> {
    Ok(sqlx::query_as(
        "SELECT id, order_id, kind, created_at, attempts FROM print_jobs \
         WHERE shop_id = ? AND status = 'queued' ORDER BY id LIMIT 1",
    )
    .bind(shop_id)
    .fetch_optional(pool)
    .await?)
}

async fn waiting(pool: &SqlitePool, shop_id: i64) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM print_jobs WHERE shop_id = ? AND status = 'queued'",
    )
    .bind(shop_id)
    .fetch_one(pool)
    .await?)
}

async fn finish(pool: &SqlitePool, job: &Job, status: &str) -> Result<()> {
    sqlx::query(
        "UPDATE print_jobs SET status = ?, error = NULL, printed_at = datetime('now') WHERE id = ?",
    )
    .bind(status)
    .bind(job.id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn record_failure(pool: &SqlitePool, job: &Job, message: &str) -> Result<()> {
    sqlx::query("UPDATE print_jobs SET attempts = attempts + 1, error = ? WHERE id = ?")
        .bind(message)
        .bind(job.id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn cleanup(pool: &SqlitePool) {
    let result = sqlx::query(
        "DELETE FROM print_jobs WHERE status IN ('done', 'skipped') \
         AND created_at < datetime('now', ?)",
    )
    .bind(format!("-{KEEP_DAYS} days"))
    .execute(pool)
    .await;
    if let Err(err) = result {
        tracing::warn!(error = %err, "could not clean old print jobs");
    }
}

/// The one worker. It never skips ahead: if the sheet at the head cannot be printed it keeps
/// trying that sheet (the cashier is told), so later queues can never come out first.
pub async fn run_worker(state: AppState) {
    loop {
        // In web mode the browser on the shop's computer does the printing (see `claim`).
        if state.print().mode == Mode::Web {
            tokio::select! {
                () = state.print_wake().notified() => {}
                () = tokio::time::sleep(Duration::from_secs(IDLE_CHECK_SECS)) => {}
            }
            continue;
        }
        match next_job(&state.pool, state.shop.id).await {
            Ok(Some(job)) => handle(&state, job).await,
            Ok(None) => {
                cleanup(&state.pool).await;
                tokio::select! {
                    () = state.print_wake().notified() => {}
                    () = tokio::time::sleep(Duration::from_secs(IDLE_CHECK_SECS)) => {}
                }
            }
            Err(err) => {
                tracing::error!(error = %format!("{err:#}"), "print queue unavailable");
                tokio::time::sleep(Duration::from_secs(RETRY_SECS)).await;
            }
        }
    }
}

async fn handle(state: &AppState, job: Job) {
    let Some(kind) = Kind::parse(&job.kind) else {
        let _ = finish(&state.pool, &job, "skipped").await;
        return;
    };
    if !state.print().ready() {
        blocked(state, &job, "ยังไม่ได้ตั้งค่าเครื่องพิมพ์").await;
        return;
    }
    let order = match orders::view_by_id(state, job.order_id).await {
        Ok(order) if order.status != "cancelled" => order,
        // Cancelled or gone: nothing to hand over, so nothing to print.
        _ => {
            let _ = finish(&state.pool, &job, "skipped").await;
            publish(state).await;
            return;
        }
    };
    let stamp = printing::stamp_of_db(&job.created_at);
    match printing::print_sheet(state, &order, kind, &stamp).await {
        Ok(()) => {
            if let Err(err) = finish(&state.pool, &job, "done").await {
                tracing::error!(error = %format!("{err:#}"), job = job.id, "could not mark job done");
            }
            publish(state).await;
            pause_between_sheets(state).await;
        }
        Err(err) => blocked(state, &job, &err.to_string()).await,
    }
}

/// Time to tear off the sheet before the next one starts; shorter when many are waiting.
/// Seconds to wait after a sheet: time to tear it off, cut short when many are waiting.
fn gap_for(left: i64, gap_secs: u64) -> u64 {
    if left == 0 {
        0
    } else if left >= BACKLOG_FAST {
        gap_secs.min(FAST_GAP_SECS)
    } else {
        gap_secs
    }
}

async fn pause_between_sheets(state: &AppState) {
    let left = waiting(&state.pool, state.shop.id).await.unwrap_or(0);
    let gap = gap_for(left, state.print().gap_secs);
    if gap > 0 {
        tokio::time::sleep(Duration::from_secs(gap)).await;
    }
}

/// One sheet handed to a browser print station.
#[derive(serde::Serialize)]
pub struct Claim {
    pub job_id: i64,
    pub kind: String,
    pub queue_label: Option<String>,
    /// Printer bytes, base64.
    pub bytes: String,
    /// How long the station waits after printing this sheet before asking for the next.
    pub pause_ms: u64,
}

#[derive(sqlx::FromRow)]
struct Head {
    id: i64,
    order_id: i64,
    kind: String,
    created_at: String,
    busy: bool,
}

/// The browser print station asks for the next sheet. Always the head of the line, so the order
/// is never broken; `None` when nothing waits or another station holds the head sheet.
pub async fn claim(state: &AppState, station: &str) -> Result<Option<Claim>> {
    use base64::Engine as _;
    state.note_station();
    loop {
        let head: Option<Head> = sqlx::query_as(
            "SELECT id, order_id, kind, created_at, \
             (lease_until IS NOT NULL AND lease_until > datetime('now') \
              AND COALESCE(lease_by, '') <> ?) AS busy \
             FROM print_jobs WHERE shop_id = ? AND status = 'queued' ORDER BY id LIMIT 1",
        )
        .bind(station)
        .bind(state.shop.id)
        .fetch_optional(&state.pool)
        .await?;
        let Some(head) = head else {
            return Ok(None);
        };
        if head.busy {
            return Ok(None);
        }
        let skip = |why: &'static str| {
            tracing::info!(job = head.id, why, "print job skipped");
        };
        let Some(kind) = Kind::parse(&head.kind) else {
            skip("unknown kind");
            finish_id(&state.pool, head.id, "skipped").await?;
            continue;
        };
        let order = match orders::view_by_id(state, head.order_id).await {
            Ok(order) if order.status != "cancelled" => order,
            _ => {
                skip("order gone or cancelled");
                finish_id(&state.pool, head.id, "skipped").await?;
                continue;
            }
        };
        let stamp = printing::stamp_of_db(&head.created_at);
        let sheet = match printing::render_sheet(state, &order, kind, &stamp).await {
            Ok(sheet) => sheet,
            Err(err) => {
                fail(state, head.id, &err.to_string()).await?;
                return Err(err);
            }
        };
        sqlx::query(
            "UPDATE print_jobs SET lease_by = ?, lease_until = datetime('now', ?) WHERE id = ?",
        )
        .bind(station)
        .bind(format!("+{LEASE_SECS} seconds"))
        .bind(head.id)
        .execute(&state.pool)
        .await?;
        let others = waiting(&state.pool, state.shop.id).await?.saturating_sub(1);
        let pause = gap_for(others, state.print().gap_secs);
        return Ok(Some(Claim {
            job_id: head.id,
            kind: head.kind,
            queue_label: order.queue_label,
            bytes: base64::engine::general_purpose::STANDARD.encode(sheet),
            pause_ms: pause * 1000,
        }));
    }
}

async fn finish_id(pool: &SqlitePool, job_id: i64, status: &str) -> Result<()> {
    sqlx::query(
        "UPDATE print_jobs SET status = ?, error = NULL, printed_at = datetime('now'), \
         lease_by = NULL, lease_until = NULL WHERE id = ?",
    )
    .bind(status)
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// The station printed the sheet.
pub async fn done(state: &AppState, job_id: i64) -> Result<()> {
    finish_id(&state.pool, job_id, "done").await?;
    publish(state).await;
    Ok(())
}

/// The station could not print the sheet; it stays at the head and the cashier is told.
pub async fn fail(state: &AppState, job_id: i64, message: &str) -> Result<()> {
    let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM print_jobs WHERE id = ?")
        .bind(job_id)
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or(0);
    sqlx::query(
        "UPDATE print_jobs SET attempts = attempts + 1, error = ?, lease_by = NULL, \
         lease_until = NULL WHERE id = ?",
    )
    .bind(message)
    .bind(job_id)
    .execute(&state.pool)
    .await?;
    if attempts == 0 {
        state.publish(Event::new("print_error", json!({ "message": message })));
    }
    publish(state).await;
    Ok(())
}

async fn blocked(state: &AppState, job: &Job, message: &str) {
    tracing::error!(
        job = job.id,
        order = job.order_id,
        error = message,
        "print failed"
    );
    if let Err(err) = record_failure(&state.pool, job, message).await {
        tracing::error!(error = %format!("{err:#}"), "could not record print failure");
    }
    if job.attempts == 0 {
        state.publish(Event::new("print_error", json!({ "message": message })));
    }
    publish(state).await;
    tokio::time::sleep(Duration::from_secs(RETRY_SECS)).await;
}

/// Gives up on the sheet at the head so the ones behind it can print.
pub async fn skip_head(state: &AppState) -> Result<()> {
    sqlx::query(
        "UPDATE print_jobs SET status = 'failed' WHERE id = \
         (SELECT MIN(id) FROM print_jobs WHERE shop_id = ? AND status = 'queued')",
    )
    .bind(state.shop.id)
    .execute(&state.pool)
    .await?;
    changed(state);
    Ok(())
}

/// Sheets that were given up on go back in line; they keep their old place (lowest ids first).
pub async fn retry_failed(state: &AppState) -> Result<()> {
    sqlx::query(
        "UPDATE print_jobs SET status = 'queued', attempts = 0, error = NULL \
         WHERE shop_id = ? AND status = 'failed'",
    )
    .bind(state.shop.id)
    .execute(&state.pool)
    .await?;
    changed(state);
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    kind: String,
    status: String,
    attempts: i64,
    error: Option<String>,
    queue_no: Option<i64>,
}

/// What the cashier sees: how many sheets wait, and whether the head of the line is stuck.
pub async fn summary(state: &AppState) -> Result<Value> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT j.id, j.kind, j.status, j.attempts, j.error, o.queue_no \
         FROM print_jobs j JOIN orders o ON o.id = j.order_id \
         WHERE j.shop_id = ? AND j.status IN ('queued', 'failed') ORDER BY j.id LIMIT ?",
    )
    .bind(state.shop.id)
    .bind(LIST_LIMIT)
    .fetch_all(&state.pool)
    .await?;
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM print_jobs WHERE shop_id = ? \
         AND status IN ('queued', 'failed') GROUP BY status",
    )
    .bind(state.shop.id)
    .fetch_all(&state.pool)
    .await?;
    let count = |status: &str| {
        counts
            .iter()
            .find(|(s, _)| s == status)
            .map_or(0, |(_, n)| *n)
    };
    let bill = state.bill();
    let label = |row: &Row| row.queue_no.map(|n| bill.queue_label(n));
    let stuck = rows
        .iter()
        .find(|r| r.status == "queued")
        .filter(|r| r.attempts > 0)
        .map(|r| {
            json!({
                "message": r.error.clone().unwrap_or_default(),
                "attempts": r.attempts,
                "queue_label": label(r),
                "kind": r.kind,
            })
        });
    let jobs: Vec<Value> = rows
        .iter()
        .map(|r| json!({ "id": r.id, "kind": r.kind, "status": r.status, "queue_label": label(r) }))
        .collect();
    Ok(json!({
        "queued": count("queued"),
        "failed": count("failed"),
        "stuck": stuck,
        "jobs": jobs,
        "web": state.print().mode == Mode::Web,
        "station_online": state.station_online(),
    }))
}

pub async fn publish(state: &AppState) {
    match summary(state).await {
        Ok(body) => state.publish(Event::new("print_state", body)),
        Err(err) => tracing::warn!(error = %format!("{err:#}"), "could not read print queue"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn pool(name: &str) -> SqlitePool {
        let path = std::env::temp_dir().join(format!("posmini-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let pool = db::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
        sqlx::query("INSERT INTO shops (id, slug, name) VALUES (1, 's', 'S')")
            .execute(&pool)
            .await
            .unwrap();
        for id in 1..=4 {
            sqlx::query(
                "INSERT INTO orders (id, shop_id, token, queue_date, queue_no, status, method, source, total_satang) \
                 VALUES (?, 1, ?, '2026-11-22', ?, 'paid', 'cash', 'pos', 1000)",
            )
            .bind(id)
            .bind(format!("t{id}"))
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    async fn add(pool: &SqlitePool, order: i64, kinds: &[Kind]) {
        let mut conn = pool.acquire().await.unwrap();
        enqueue(&mut conn, 1, order, kinds).await.unwrap();
    }

    #[tokio::test]
    async fn sheets_come_out_in_the_order_they_were_queued() {
        let pool = pool("printq-order").await;
        add(&pool, 1, &[Kind::Ticket, Kind::Slip]).await;
        add(&pool, 2, &[Kind::Ticket, Kind::Slip]).await;
        add(&pool, 3, &[Kind::Ticket]).await;
        let mut seen = Vec::new();
        while let Some(job) = next_job(&pool, 1).await.unwrap() {
            seen.push((job.order_id, job.kind.clone()));
            finish(&pool, &job, "done").await.unwrap();
        }
        assert_eq!(
            seen,
            [
                (1, "ticket".to_string()),
                (1, "slip".to_string()),
                (2, "ticket".to_string()),
                (2, "slip".to_string()),
                (3, "ticket".to_string()),
            ]
        );
        assert_eq!(waiting(&pool, 1).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_failing_sheet_stays_at_the_head_instead_of_being_overtaken() {
        let pool = pool("printq-block").await;
        add(&pool, 1, &[Kind::Ticket]).await;
        add(&pool, 2, &[Kind::Ticket]).await;
        let head = next_job(&pool, 1).await.unwrap().unwrap();
        record_failure(&pool, &head, "offline").await.unwrap();
        let again = next_job(&pool, 1).await.unwrap().unwrap();
        assert_eq!((again.id, again.order_id, again.attempts), (head.id, 1, 1));
    }

    #[tokio::test]
    async fn given_up_sheets_return_to_their_old_place() {
        let pool = pool("printq-retry").await;
        add(&pool, 1, &[Kind::Ticket]).await;
        add(&pool, 2, &[Kind::Ticket]).await;
        let head = next_job(&pool, 1).await.unwrap().unwrap();
        sqlx::query("UPDATE print_jobs SET status = 'failed' WHERE id = ?")
            .bind(head.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(next_job(&pool, 1).await.unwrap().unwrap().order_id, 2);
        sqlx::query("UPDATE print_jobs SET status = 'queued' WHERE status = 'failed'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(next_job(&pool, 1).await.unwrap().unwrap().order_id, 1);
    }

    #[tokio::test]
    async fn calling_ahead_is_detected() {
        let pool = pool("printq-call").await;
        // Queues 1..4 are all paid and waiting.
        assert_eq!(
            orders::earlier_waiting(&pool, 1, "2026-11-22", 3)
                .await
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            orders::earlier_waiting(&pool, 1, "2026-11-22", 1)
                .await
                .unwrap(),
            None
        );
        sqlx::query("UPDATE orders SET status = 'called' WHERE queue_no < 3")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            orders::earlier_waiting(&pool, 1, "2026-11-22", 3)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            orders::earlier_waiting(&pool, 1, "2026-11-22", 4)
                .await
                .unwrap(),
            Some(3)
        );
    }
}
