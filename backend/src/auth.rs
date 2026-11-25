use std::net::SocketAddr;

use anyhow::{Result, anyhow};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    Form,
    extract::{ConnectInfo, FromRequestParts, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, state::AppState};

const COOKIE_NAME: &str = "sid";
const LOGIN_MAX_FAILURES: usize = 5;
const LOGIN_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

pub fn hash_password(password: &str) -> Result<String> {
    let hash: PasswordHash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow!("hash password: {e}"))?;
    Ok(hash.to_string())
}

fn verify_password(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

fn session_id(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.to_string())
}

/// What an account may do. A cashier sells, takes money and calls queues; everything that sets the
/// shop up, shows the takings or changes who can log in needs an admin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Cashier,
}

impl Role {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "admin" => Some(Self::Admin),
            "cashier" => Some(Self::Cashier),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Cashier => "cashier",
        }
    }
}

/// The account behind a login session.
#[derive(Clone, Debug, Serialize)]
pub struct SessionUser {
    pub id: i64,
    pub username: String,
    pub role: Role,
}

pub fn session_user(state: &AppState, headers: &HeaderMap) -> Option<SessionUser> {
    session_id(headers).and_then(|sid| state.session_user(&sid))
}

/// Any logged-in account.
pub struct Authed(pub SessionUser);

impl FromRequestParts<AppState> for Authed {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        session_user(state, &parts.headers)
            .map(Self)
            .ok_or(ApiError::Unauthorized)
    }
}

/// A logged-in account with the admin role (a cashier gets 403).
pub struct Admin(pub SessionUser);

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match session_user(state, &parts.headers) {
            Some(user) if user.role == Role::Admin => Ok(Self(user)),
            Some(_) => Err(ApiError::Forbidden),
            None => Err(ApiError::Unauthorized),
        }
    }
}

pub fn client_key(headers: &HeaderMap, peer: SocketAddr, trust_proxy: bool) -> String {
    // Behind Cloudflare / Railway the TCP peer is the proxy; these headers carry the visitor.
    if trust_proxy
        && let Some(ip) = ["cf-connecting-ip", "x-real-ip"]
            .iter()
            .find_map(|name| headers.get(*name).and_then(|v| v.to_str().ok()))
            .map(str::trim)
            .filter(|ip| !ip.is_empty())
    {
        return ip.to_string();
    }
    peer.ip().to_string()
}

#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
}

fn cookie_header(state: &AppState, value: &str, max_age: u64) -> HeaderValue {
    let secure = if state.cfg.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    let text =
        format!("{COOKIE_NAME}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}");
    HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<Response, ApiError> {
    let key = format!(
        "login:{}",
        client_key(&headers, peer, state.cfg.trust_proxy)
    );
    if state.is_limited(&key, LOGIN_MAX_FAILURES, LOGIN_WINDOW) {
        return Err(ApiError::TooMany);
    }
    let account: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT id, username, password_hash, role FROM users          WHERE shop_id = ? AND username = ? AND active = 1",
    )
    .bind(state.shop.id)
    .bind(form.username.trim())
    .fetch_optional(&state.pool)
    .await?;

    let user = account.and_then(|(id, username, hash, role)| {
        let role = Role::parse(&role)?;
        verify_password(&form.password, &hash).then_some(SessionUser { id, username, role })
    });
    let Some(user) = user else {
        state.record_hit(&key);
        return Ok(Redirect::to("/login?error=1").into_response());
    };
    let sid = state.create_session(user);
    let mut response = Redirect::to("/pos").into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, cookie_header(&state, &sid, 12 * 3600));
    Ok(response)
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(sid) = session_id(&headers) {
        state.drop_session(&sid);
    }
    let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, "/login")]).into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, cookie_header(&state, "", 0));
    response
}
