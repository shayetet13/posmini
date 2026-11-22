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
use serde::Deserialize;

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

pub fn is_authed(state: &AppState, headers: &HeaderMap) -> bool {
    session_id(headers).is_some_and(|sid| state.session_valid(&sid))
}

pub struct Authed;

impl FromRequestParts<AppState> for Authed {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if is_authed(state, &parts.headers) {
            Ok(Self)
        } else {
            Err(ApiError::Unauthorized)
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
    let stored: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE shop_id = ? AND username = ?")
            .bind(state.shop.id)
            .bind(form.username.trim())
            .fetch_optional(&state.pool)
            .await?;

    let valid = stored.is_some_and(|hash| verify_password(&form.password, &hash));
    if !valid {
        state.record_hit(&key);
        return Ok(Redirect::to("/login?error=1").into_response());
    }
    let sid = state.create_session();
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
