//! End-to-end tests through the real router: login, orders, the print queue and the browser
//! print station, queue calling order. They run against a throw-away SQLite database, so they
//! need no network, no printer and no Thai font (print style is set to text).

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{
    bill::BillSettings,
    config::Config,
    db,
    printing::PrintSettings,
    settings::PaySettings,
    state::{AppState, PayRuntime},
};

const ADMIN_PASSWORD: &str = "test-password-123";

fn config(serve_ui: bool, db_path: &Path) -> Config {
    Config {
        database_url: format!("sqlite://{}?mode=rwc", db_path.display()),
        bind_addr: "127.0.0.1:0".into(),
        shop_slug: "main".into(),
        shop_name: "ร้านทดสอบ".into(),
        admin_user: "admin".into(),
        admin_password: ADMIN_PASSWORD.into(),
        cookie_secure: false,
        trust_proxy: false,
        serve_ui,
        pay_defaults: PaySettings::default(),
        print_dump: None,
        print_font: None,
    }
}

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

struct Client {
    app: Router,
    cookie: Option<String>,
    db: PathBuf,
}

impl Drop for Client {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.db.display()));
        }
    }
}

impl Client {
    async fn new(name: &str, serve_ui: bool) -> Self {
        let db_path =
            std::env::temp_dir().join(format!("posmini-http-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db_path);
        let cfg = config(serve_ui, &db_path);
        let pool = db::connect(&cfg.database_url).await.unwrap();
        let shop = db::seed(&pool, &cfg).await.unwrap();
        let pay = PayRuntime::build(PaySettings::default()).unwrap();
        let state = AppState::new(
            pool,
            cfg,
            shop,
            pay,
            PrintSettings::default(),
            BillSettings::default(),
        );
        Self {
            app: crate::router(state),
            cookie: None,
            db: db_path,
        }
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        form: Option<&str>,
    ) -> Reply {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(cookie) = &self.cookie {
            req = req.header(header::COOKIE, cookie);
        }
        let body = match (body, form) {
            (Some(json), _) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(json.to_string())
            }
            (None, Some(form)) => {
                req = req.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(form.to_string())
            }
            (None, None) => Body::empty(),
        };
        let mut req = req.body(body).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        Reply {
            status: parts.status,
            headers: parts.headers,
            body: body.collect().await.unwrap().to_bytes().to_vec(),
        }
    }

    async fn get(&self, uri: &str) -> Reply {
        self.send(Method::GET, uri, None, None).await
    }

    async fn post(&self, uri: &str, body: Value) -> Reply {
        self.send(Method::POST, uri, Some(body), None).await
    }

    async fn put(&self, uri: &str, body: Value) -> Reply {
        self.send(Method::PUT, uri, Some(body), None).await
    }

    async fn login(&mut self) {
        let form = format!("username=admin&password={ADMIN_PASSWORD}");
        let res = self.send(Method::POST, "/login", None, Some(&form)).await;
        assert_eq!(res.status, StatusCode::SEE_OTHER);
        let cookie = res
            .headers
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("login sets a cookie")
            .split(';')
            .next()
            .unwrap()
            .to_string();
        self.cookie = Some(cookie);
    }

    /// Prints with the browser station and as plain text, so no font or printer is needed.
    async fn web_text_printing(&self) {
        let res = self
            .put(
                "/api/print/settings",
                json!({
                    "mode": "web", "target": "", "width": 58, "codepage": 26, "ticket": true,
                    "cut": true, "style": "text", "order_slip": true, "head_dots": 0,
                    "shift": 0, "gap_secs": 0
                }),
            )
            .await;
        assert_eq!(res.status, StatusCode::OK, "{}", res.text());
    }

    async fn first_product(&self) -> i64 {
        self.get("/api/products").await.json()[0]["id"]
            .as_i64()
            .unwrap()
    }

    async fn cash_order(&self, product: i64, received_satang: Option<i64>) -> Reply {
        let mut body = json!({ "method": "cash", "items": [{ "product_id": product, "qty": 1 }] });
        if let Some(received) = received_satang {
            body["cash_received_satang"] = json!(received);
        }
        self.post("/api/orders", body).await
    }
}

#[tokio::test]
async fn health_and_shop_script_are_public() {
    let client = Client::new("public", true).await;
    let health = client.get("/health").await;
    assert_eq!(
        (health.status, health.text().as_str()),
        (StatusCode::OK, "ok")
    );
    let shop = client.get("/shop.js").await;
    assert_eq!(shop.status, StatusCode::OK);
    let js = shop.text();
    assert!(js.contains(r#"window.SHOP = "ร้านทดสอบ";"#), "{js}");
    assert!(js.contains(r#"window.SLUG = "main";"#));
    assert!(js.contains(r#"window.SHOP_INITIAL = "ร";"#));
}

#[tokio::test]
async fn pages_are_served_only_when_ui_is_on() {
    let with_ui = Client::new("ui-on", true).await;
    let pos = with_ui.get("/pos").await;
    assert_eq!(pos.status, StatusCode::OK);
    assert!(pos.text().contains("/static/pos.js"));
    assert_eq!(with_ui.get("/static/app.css").await.status, StatusCode::OK);
    drop(with_ui);

    let api_only = Client::new("ui-off", false).await;
    for path in ["/pos", "/static/app.css", "/display/main"] {
        assert_eq!(
            api_only.get(path).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    // Only the login form POST exists on the API; the form itself comes from the frontend.
    assert_ne!(api_only.get("/login").await.status, StatusCode::OK);
    assert_eq!(api_only.get("/health").await.status, StatusCode::OK);
}

#[tokio::test]
async fn api_needs_a_login_and_a_wrong_password_is_refused() {
    let mut client = Client::new("auth", false).await;
    for path in [
        "/api/orders/active",
        "/api/settings",
        "/api/bill/settings",
        "/api/print/queue",
        "/api/dashboard?period=day&date=2026-11-22",
    ] {
        assert_eq!(
            client.get(path).await.status,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    let bad = client
        .send(
            Method::POST,
            "/login",
            None,
            Some("username=admin&password=nope"),
        )
        .await;
    assert_eq!(bad.status, StatusCode::SEE_OTHER);
    assert_eq!(bad.headers[header::LOCATION], "/login?error=1");
    assert!(bad.headers.get(header::SET_COOKIE).is_none());

    client.login().await;
    assert_eq!(
        client.get("/api/orders/active").await.status,
        StatusCode::OK
    );
    assert_eq!(client.get("/api/settings").await.status, StatusCode::OK);
}

#[tokio::test]
async fn login_is_rate_limited_after_repeated_failures() {
    let client = Client::new("ratelimit", false).await;
    let mut last = StatusCode::OK;
    for _ in 0..12 {
        last = client
            .send(
                Method::POST,
                "/login",
                None,
                Some("username=admin&password=wrong"),
            )
            .await
            .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn cash_order_records_the_amount_received_and_the_change() {
    let mut client = Client::new("cash", false).await;
    client.login().await;
    let product = client.first_product().await;

    let order = client.cash_order(product, Some(100_000)).await;
    assert_eq!(order.status, StatusCode::OK, "{}", order.text());
    let order = order.json();
    assert_eq!(order["status"], "paid");
    assert_eq!(order["queue_label"], "A001");
    let total = order["total_satang"].as_i64().unwrap();
    assert_eq!(order["cash_received_satang"], 100_000);
    assert_eq!(order["change_satang"].as_i64().unwrap(), 100_000 - total);

    let short = client.cash_order(product, Some(total - 1)).await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);
    let absurd = client.cash_order(product, Some(99_999_999)).await;
    assert_eq!(absurd.status, StatusCode::BAD_REQUEST);
    // Without an amount the sale still goes through (no change shown).
    let plain = client.cash_order(product, None).await;
    assert_eq!(plain.status, StatusCode::OK);
    assert_eq!(plain.json()["change_satang"], Value::Null);
    assert_eq!(plain.json()["queue_label"], "A002");
}

#[tokio::test]
async fn queue_numbers_follow_the_bill_settings() {
    let mut client = Client::new("queue-format", false).await;
    client.login().await;
    let product = client.first_product().await;
    let saved = client
        .put(
            "/api/bill/settings",
            json!({
                "shop_name": "", "header": "", "footer": "", "ticket_title": "",
                "slip_title": "", "ticket_note": "", "queue_prefix": "B", "queue_digits": 2,
                "show_time": true, "show_method": true, "show_order_no": true
            }),
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
    let order = client.cash_order(product, None).await.json();
    assert_eq!(order["queue_label"], "B01");
    let by_token = client
        .get(&format!("/api/q/{}", order["token"].as_str().unwrap()))
        .await
        .json();
    assert_eq!(by_token["queue_label"], "B01");
    assert!(
        by_token["bill"]["shop"]
            .as_str()
            .unwrap()
            .contains("ร้านทดสอบ")
    );
}

async fn claim(client: &Client, station: &str) -> Option<Value> {
    let res = client
        .post("/api/print/station/claim", json!({ "station": station }))
        .await;
    match res.status {
        StatusCode::NO_CONTENT => None,
        StatusCode::OK => Some(res.json()),
        other => panic!("claim failed: {other} {}", res.text()),
    }
}

#[tokio::test]
async fn the_print_station_gets_sheets_strictly_in_queue_order() {
    let mut client = Client::new("station", false).await;
    client.login().await;
    client.web_text_printing().await;
    let product = client.first_product().await;
    client.cash_order(product, None).await;
    client.cash_order(product, None).await;

    let queue = client.get("/api/print/queue").await.json();
    assert_eq!(queue["queued"], 4);
    assert_eq!(queue["web"], true);

    let mut seen = Vec::new();
    while let Some(sheet) = claim(&client, "tablet-1").await {
        // A second station must wait: the head sheet is leased and the order may not be broken.
        assert!(claim(&client, "tablet-2").await.is_none());
        assert!(!sheet["bytes"].as_str().unwrap().is_empty());
        seen.push(format!(
            "{} {}",
            sheet["queue_label"].as_str().unwrap(),
            sheet["kind"].as_str().unwrap()
        ));
        let done = client
            .post(
                "/api/print/station/done",
                json!({ "job_id": sheet["job_id"] }),
            )
            .await;
        assert_eq!(done.status, StatusCode::OK);
    }
    assert_eq!(
        seen,
        ["A001 ticket", "A001 slip", "A002 ticket", "A002 slip"]
    );
    assert_eq!(client.get("/api/print/queue").await.json()["queued"], 0);
}

#[tokio::test]
async fn a_failed_sheet_stays_at_the_head_and_is_reported() {
    let mut client = Client::new("station-fail", false).await;
    client.login().await;
    client.web_text_printing().await;
    let product = client.first_product().await;
    client.cash_order(product, None).await;
    client.cash_order(product, None).await;

    let first = claim(&client, "tablet-1").await.unwrap();
    let failed = client
        .post(
            "/api/print/station/fail",
            json!({ "job_id": first["job_id"], "message": "printer offline" }),
        )
        .await;
    assert_eq!(failed.status, StatusCode::OK);
    let queue = client.get("/api/print/queue").await.json();
    assert_eq!(queue["stuck"]["message"], "printer offline");
    assert_eq!(queue["stuck"]["queue_label"], "A001");

    // Nothing from A002 may jump ahead of the stuck sheet.
    let again = claim(&client, "tablet-1").await.unwrap();
    assert_eq!(again["job_id"], first["job_id"]);
    assert_eq!(again["queue_label"], "A001");
}

#[tokio::test]
async fn the_station_endpoints_need_web_mode_and_a_login() {
    let mut client = Client::new("station-guard", false).await;
    let anonymous = client
        .post("/api/print/station/claim", json!({ "station": "x" }))
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    client.login().await;
    // Printing is off by default, so a station has nothing to do.
    let off = client
        .post("/api/print/station/claim", json!({ "station": "x" }))
        .await;
    assert_eq!(off.status, StatusCode::BAD_REQUEST);
    client.web_text_printing().await;
    let blank = client
        .post("/api/print/station/claim", json!({ "station": "" }))
        .await;
    assert_eq!(blank.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn web_mode_test_pages_come_back_as_bytes_for_the_browser() {
    let mut client = Client::new("station-test", false).await;
    client.login().await;
    client.web_text_printing().await;
    for kind in ["ticket", "slip", "position", "codepages"] {
        let res = client
            .post("/api/print/test", json!({ "kind": kind }))
            .await;
        assert_eq!(res.status, StatusCode::OK, "{kind}: {}", res.text());
        let body = res.json();
        assert_eq!(body["web"], true);
        assert!(
            body["bytes"].as_str().is_some_and(|b| b.len() > 20),
            "{kind}"
        );
    }
}

#[tokio::test]
async fn calling_a_queue_out_of_turn_needs_confirmation() {
    let mut client = Client::new("calling", false).await;
    client.login().await;
    let product = client.first_product().await;
    let first = client.cash_order(product, None).await.json();
    client.cash_order(product, None).await;
    let third = client.cash_order(product, None).await.json();
    let id = |order: &Value| order["id"].as_i64().unwrap();

    let ahead = client
        .post(&format!("/api/orders/{}/call", id(&third)), json!({}))
        .await;
    assert_eq!(ahead.status, StatusCode::CONFLICT);
    assert!(ahead.json()["error"].as_str().unwrap().contains("A001"));
    let forced = client
        .post(
            &format!("/api/orders/{}/call?force=true", id(&third)),
            json!({}),
        )
        .await;
    assert_eq!(forced.status, StatusCode::OK);

    // "Call next" always takes the longest-waiting queue.
    let next = client.post("/api/orders/call-next", json!({})).await;
    assert_eq!(next.status, StatusCode::OK);
    assert_eq!(next.json()["order_id"], id(&first));

    let active = client.get("/api/orders/active").await.json();
    let orders = active.as_array().unwrap();
    let labels: Vec<&str> = orders
        .iter()
        .map(|o| o["queue_label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["A001", "A002", "A003"], "listed by queue number");
    let status_of =
        |label: &str| orders.iter().find(|o| o["queue_label"] == label).unwrap()["status"].clone();
    assert_eq!(status_of("A001"), "called");
    assert_eq!(status_of("A002"), "paid");
    assert_eq!(status_of("A003"), "called");
}
