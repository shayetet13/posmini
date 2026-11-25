//! Staff accounts. The admin makes cashier accounts (and more admins); a cashier can sell, take
//! money and call queues but cannot change settings, see the takings or manage accounts.

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    audit::{self, action},
    auth::{self, Admin, Authed, Role},
    error::{ApiError, ApiResult},
    state::AppState,
};

const NAME_MIN: usize = 3;
const NAME_MAX: usize = 30;
const PASSWORD_MIN: usize = 8;
const PASSWORD_MAX: usize = 100;
const LAST_ADMIN: &str = "ต้องมีแอดมินที่ใช้งานอยู่อย่างน้อย 1 คน";
const BAD_ROLE: &str = "ตำแหน่งต้องเป็น admin หรือ cashier";

/// Who am I: the pages use this to show or hide what the account may not use.
pub async fn me(Authed(user): Authed) -> Json<Value> {
    Json(json!({ "id": user.id, "username": user.username, "role": user.role }))
}

#[derive(sqlx::FromRow)]
struct Account {
    id: i64,
    username: String,
    role: String,
    active: i64,
    created_at: Option<String>,
}

pub async fn list(_admin: Admin, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let rows: Vec<Account> = sqlx::query_as(
        "SELECT id, username, role, active, created_at FROM users WHERE shop_id = ? ORDER BY id",
    )
    .bind(state.shop.id)
    .fetch_all(&state.pool)
    .await?;
    let users: Vec<Value> = rows
        .into_iter()
        .map(|a| {
            json!({ "id": a.id, "username": a.username, "role": a.role,
                    "active": a.active == 1, "created_at": a.created_at })
        })
        .collect();
    Ok(Json(json!(users)))
}

fn valid_name(name: &str) -> bool {
    let n = name.chars().count();
    (NAME_MIN..=NAME_MAX).contains(&n)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn valid_password(password: &str) -> ApiResult<()> {
    let n = password.chars().count();
    if (PASSWORD_MIN..=PASSWORD_MAX).contains(&n) {
        Ok(())
    } else {
        Err(ApiError::BadRequest("รหัสผ่านต้องยาว 8-100 ตัวอักษร"))
    }
}

#[derive(Deserialize)]
pub struct NewAccount {
    username: String,
    password: String,
    role: String,
}

pub async fn create(
    Admin(admin): Admin,
    State(state): State<AppState>,
    Json(req): Json<NewAccount>,
) -> ApiResult<Json<Value>> {
    let name = req.username.trim();
    if !valid_name(name) {
        return Err(ApiError::BadRequest(
            "ชื่อผู้ใช้ต้องยาว 3-30 ตัว ใช้ได้เฉพาะ a-z A-Z 0-9 . _ -",
        ));
    }
    valid_password(&req.password)?;
    let role = Role::parse(&req.role).ok_or(ApiError::BadRequest(BAD_ROLE))?;
    let hash = auth::hash_password(&req.password)?;
    let made: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO users (shop_id, username, password_hash, role, created_at) \
         VALUES (?, ?, ?, ?, datetime('now')) RETURNING id",
    )
    .bind(state.shop.id)
    .bind(name)
    .bind(hash)
    .bind(role.as_str())
    .fetch_one(&state.pool)
    .await;
    let id = match made {
        Ok(id) => id,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::Conflict("ชื่อผู้ใช้นี้มีอยู่แล้ว".to_string()));
        }
        Err(e) => return Err(e.into()),
    };
    let detail = format!("{name} ({})", role.as_str());
    audit::record(&state, &admin, action::STAFF_ADDED, None, &detail).await;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct Change {
    role: Option<String>,
    active: Option<bool>,
    password: Option<String>,
}

/// After the change there must still be an active admin, or nobody could manage the shop.
async fn another_admin_remains(state: &AppState, leaving: i64) -> ApiResult<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE shop_id = ? AND role = 'admin' AND active = 1 AND id <> ?",
    )
    .bind(state.shop.id)
    .bind(leaving)
    .fetch_one(&state.pool)
    .await?;
    Ok(n > 0)
}

async fn account(state: &AppState, id: i64) -> ApiResult<Account> {
    sqlx::query_as(
        "SELECT id, username, role, active, created_at FROM users WHERE id = ? AND shop_id = ?",
    )
    .bind(id)
    .bind(state.shop.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)
}

pub async fn update(
    Admin(admin): Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<Change>,
) -> ApiResult<Json<Value>> {
    let target = account(&state, id).await?;
    let was_admin = target.role == "admin" && target.active == 1;
    let role = match req.role.as_deref() {
        Some(text) => Role::parse(text).ok_or(ApiError::BadRequest(BAD_ROLE))?,
        None => Role::parse(&target.role).unwrap_or(Role::Cashier),
    };
    let active = req.active.unwrap_or(target.active == 1);
    if was_admin && (role != Role::Admin || !active) && !another_admin_remains(&state, id).await? {
        return Err(ApiError::BadRequest(LAST_ADMIN));
    }
    if let Some(password) = &req.password {
        valid_password(password)?;
        let hash = auth::hash_password(password)?;
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ? AND shop_id = ?")
            .bind(hash)
            .bind(id)
            .bind(state.shop.id)
            .execute(&state.pool)
            .await?;
    }
    sqlx::query("UPDATE users SET role = ?, active = ? WHERE id = ? AND shop_id = ?")
        .bind(role.as_str())
        .bind(i64::from(active))
        .bind(id)
        .bind(state.shop.id)
        .execute(&state.pool)
        .await?;
    // A changed role, a switched-off account or a new password ends the sessions it had.
    state.drop_sessions_of(id);
    let mut detail = format!(
        "{} เป็น {}{}",
        target.username,
        role.as_str(),
        if active {
            ""
        } else {
            " (ปิดใช้งาน)"
        }
    );
    if req.password.is_some() {
        detail.push_str(" เปลี่ยนรหัสผ่าน");
    }
    audit::record(&state, &admin, action::STAFF_CHANGED, None, &detail).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn remove(
    Admin(admin): Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let target = account(&state, id).await?;
    if id == admin.id {
        return Err(ApiError::BadRequest("ลบบัญชีที่กำลังใช้อยู่ไม่ได้"));
    }
    if target.role == "admin" && target.active == 1 && !another_admin_remains(&state, id).await? {
        return Err(ApiError::BadRequest(LAST_ADMIN));
    }
    sqlx::query("DELETE FROM users WHERE id = ? AND shop_id = ?")
        .bind(id)
        .bind(state.shop.id)
        .execute(&state.pool)
        .await?;
    state.drop_sessions_of(id);
    audit::record(
        &state,
        &admin,
        action::STAFF_REMOVED,
        None,
        &target.username,
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}
