//! Backups of the whole database. `VACUUM INTO` writes a consistent copy while the shop keeps
//! selling. The admin downloads one with a button; the server also makes one by itself about once
//! a day and keeps the newest few, so a bad day can be rolled back to the night before.
//!
//! Copies kept on the same disk protect against a mistake, not against losing the disk: download
//! one now and then, or put `BACKUP_DIR` on a different volume.

use std::{
    path::{Path as FsPath, PathBuf},
    time::{Duration, SystemTime},
};

#[cfg(test)]
use anyhow::bail;
use anyhow::{Context, Result};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, SqlitePool};

use crate::{
    audit::{self, action},
    auth::Admin,
    config::Config,
    error::{ApiError, ApiResult},
    state::AppState,
};

const PREFIX: &str = "posmini-";
const SUFFIX: &str = ".db";
const KEEP_NEWEST: usize = 14;
const AUTO_EVERY: Duration = Duration::from_secs(24 * 3600);
const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(90);
const BANGKOK_OFFSET_HOURS: i64 = 7;

/// `BACKUP_DIR`, or a `backups` folder next to the database file.
pub fn dir_for(cfg: &Config) -> PathBuf {
    if let Some(dir) = &cfg.backup_dir {
        return dir.clone();
    }
    let file = cfg
        .database_url
        .trim_start_matches("sqlite://")
        .trim_start_matches("sqlite:")
        .split('?')
        .next()
        .unwrap_or_default();
    FsPath::new(file)
        .parent()
        .map(|p| p.join("backups"))
        .unwrap_or_else(|| PathBuf::from("backups"))
}

fn stamp() -> String {
    (chrono::Utc::now() + chrono::Duration::hours(BANGKOK_OFFSET_HOURS))
        .format("%Y%m%d-%H%M%S")
        .to_string()
}

/// A file this module made (and so may offer for download or delete).
fn is_backup_name(name: &str) -> bool {
    name.starts_with(PREFIX)
        && name.ends_with(SUFFIX)
        && name.len() < 60
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
}

/// Writes a consistent copy of the database into `dir` and returns its path.
pub async fn snapshot(pool: &SqlitePool, dir: &FsPath) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("create {}", dir.display()))?;
    let mut path = dir.join(format!("{PREFIX}{}{SUFFIX}", stamp()));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{PREFIX}{}-{n}{SUFFIX}", stamp()));
        n += 1;
    }
    let target = path.to_string_lossy().replace('\'', "''");
    sqlx::query(AssertSqlSafe(format!("VACUUM INTO '{target}'")))
        .execute(pool)
        .await
        .context("write the backup copy")?;
    Ok(path)
}

/// Deletes all but the newest `keep` backup files; returns how many went.
pub fn prune(dir: &FsPath, keep: usize) -> Result<usize> {
    let mut files = files_in(dir)?;
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    let mut removed = 0;
    for (path, _, _) in files.into_iter().skip(keep) {
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// (path, modified time, size) of the backups in `dir`, in no particular order.
fn files_in(dir: &FsPath) -> Result<Vec<(PathBuf, SystemTime, u64)>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_backup_name(&name) {
            continue;
        }
        let meta = entry.metadata()?;
        out.push((
            entry.path(),
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            meta.len(),
        ));
    }
    Ok(out)
}

fn newest_age(dir: &FsPath) -> Option<Duration> {
    files_in(dir)
        .ok()?
        .into_iter()
        .map(|(_, at, _)| at)
        .max()
        .and_then(|at| at.elapsed().ok())
}

/// Makes a backup by itself when the newest one is a day old (or there is none).
pub async fn run_scheduler(state: AppState) {
    let dir = dir_for(&state.cfg);
    tokio::time::sleep(FIRST_CHECK_AFTER).await;
    loop {
        if newest_age(&dir).is_none_or(|age| age >= AUTO_EVERY) {
            match snapshot(&state.pool, &dir).await {
                Ok(path) => {
                    tracing::info!(file = %path.display(), "daily backup written");
                    if let Err(err) = prune(&dir, KEEP_NEWEST) {
                        tracing::warn!(error = %err, "could not prune old backups");
                    }
                }
                Err(err) => tracing::error!(error = %format!("{err:#}"), "daily backup failed"),
            }
        }
        tokio::time::sleep(CHECK_EVERY).await;
    }
}

fn attachment(name: &str, bytes: Vec<u8>) -> Response {
    let disposition = format!("attachment; filename=\"{name}\"");
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&disposition)
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        bytes,
    )
        .into_response()
}

/// Makes a fresh backup right now and sends it as a download.
pub async fn download(Admin(admin): Admin, State(state): State<AppState>) -> ApiResult<Response> {
    let dir = dir_for(&state.cfg);
    let path = snapshot(&state.pool, &dir).await?;
    prune(&dir, KEEP_NEWEST)?;
    let bytes = tokio::fs::read(&path)
        .await
        .context("read the backup copy")?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    audit::record(
        &state,
        &admin,
        action::BACKUP,
        None,
        &format!("ดาวน์โหลดไฟล์สำรอง {name}"),
    )
    .await;
    Ok(attachment(&name, bytes))
}

/// The backups on the server, newest first.
pub async fn list(_admin: Admin, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let dir = dir_for(&state.cfg);
    let mut files = files_in(&dir)?;
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    let rows: Vec<Value> = files
        .into_iter()
        .map(|(path, at, size)| {
            let at: chrono::DateTime<chrono::Utc> = at.into();
            json!({
                "name": path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "size": size,
                "at": (at + chrono::Duration::hours(BANGKOK_OFFSET_HOURS)).format("%Y-%m-%d %H:%M").to_string(),
            })
        })
        .collect();
    Ok(Json(json!({ "files": rows, "keep": KEEP_NEWEST })))
}

/// One kept backup as a download. Only names this module makes are served, never a path.
pub async fn file(
    Admin(admin): Admin,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Response> {
    if !is_backup_name(&name) {
        return Err(ApiError::NotFound);
    }
    let path = dir_for(&state.cfg).join(&name);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    audit::record(
        &state,
        &admin,
        action::BACKUP,
        None,
        &format!("ดาวน์โหลดไฟล์สำรอง {name}"),
    )
    .await;
    Ok(attachment(&name, bytes))
}

/// Fails unless `bytes` is a SQLite database.
#[cfg(test)]
pub fn looks_like_sqlite(bytes: &[u8]) -> Result<()> {
    if bytes.starts_with(b"SQLite format 3\0") {
        Ok(())
    } else {
        bail!("not a SQLite database file")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_names_this_module_makes_count_as_backups() {
        assert!(is_backup_name("posmini-20261125-031500.db"));
        assert!(is_backup_name("posmini-20261125-031500-2.db"));
        for bad in [
            "",
            "posmini-.txt",
            "other.db",
            "../posmini-1.db",
            "posmini-1/2.db",
            "posmini-a b.db",
        ] {
            assert!(!is_backup_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_newest_files_are_kept_and_strangers_are_left_alone() {
        let dir = std::env::temp_dir().join(format!("posmini-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in 1..=5 {
            std::fs::write(dir.join(format!("posmini-{n}.db")), b"x").unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        std::fs::write(dir.join("notes.txt"), b"keep me").unwrap();

        assert_eq!(prune(&dir, 2).unwrap(), 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["notes.txt", "posmini-4.db", "posmini-5.db"]);
        assert!(newest_age(&dir).unwrap() < Duration::from_secs(60));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(newest_age(&dir).is_none(), "no folder, no backup");
    }

    #[test]
    fn the_folder_defaults_to_next_to_the_database() {
        let mut cfg = Config {
            database_url: "sqlite:///data/posmini.db?mode=rwc".into(),
            bind_addr: String::new(),
            shop_slug: String::new(),
            shop_name: String::new(),
            admin_user: String::new(),
            admin_password: String::new(),
            cookie_secure: false,
            trust_proxy: false,
            serve_ui: false,
            pay_defaults: Default::default(),
            print_dump: None,
            print_font: None,
            backup_dir: None,
        };
        assert_eq!(dir_for(&cfg), FsPath::new("/data").join("backups"));
        cfg.backup_dir = Some(PathBuf::from("/mnt/other"));
        assert_eq!(dir_for(&cfg), PathBuf::from("/mnt/other"));
    }
}
