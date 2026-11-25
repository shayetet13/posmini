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
    backup,
    bill::BillSettings,
    config::Config,
    db, orders,
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
        backup_dir: Some(PathBuf::from(format!("{}-backups", db_path.display()))),
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
    /// The same state the router uses, for tests that need to look at or prepare the database.
    state: AppState,
    cookie: Option<String>,
    db: PathBuf,
}

impl Drop for Client {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.db.display()));
        }
        let _ = std::fs::remove_dir_all(format!("{}-backups", self.db.display()));
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
            app: crate::router(state.clone()),
            state,
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
        self.login_as("admin", ADMIN_PASSWORD).await;
    }

    /// A second visitor to the same app (same router and database) with no login yet.
    fn another_session(&self) -> Client {
        Client {
            app: self.app.clone(),
            state: self.state.clone(),
            cookie: None,
            db: std::env::temp_dir().join("posmini-borrowed-session.db"),
        }
    }

    /// Logs in as any account; panics if the login is refused.
    async fn login_as(&mut self, username: &str, password: &str) {
        assert!(
            self.try_login(username, password).await,
            "login refused for {username}"
        );
    }

    /// Whether the login worked (a good one sets the cookie, a bad one goes back to the form).
    async fn try_login(&mut self, username: &str, password: &str) -> bool {
        let form = format!("username={username}&password={password}");
        let res = self.send(Method::POST, "/login", None, Some(&form)).await;
        assert_eq!(res.status, StatusCode::SEE_OTHER);
        let went_to = res
            .headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if went_to.contains("error") {
            return false;
        }
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
        true
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
async fn the_web_app_manifest_makes_the_site_installable() {
    let client = Client::new("manifest", false).await;
    let res = client.get("/manifest.webmanifest").await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(
        res.headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/manifest+json")
    );
    let manifest = res.json();
    assert_eq!(manifest["name"], "ร้านทดสอบ POS");
    assert_eq!(manifest["start_url"], "/pos");
    assert_eq!(manifest["scope"], "/");
    assert_eq!(manifest["display"], "standalone");
    let icons = manifest["icons"].as_array().unwrap();
    let has = |size: &str, purpose: &str| {
        icons
            .iter()
            .any(|i| i["sizes"] == size && i["purpose"] == purpose)
    };
    assert!(has("192x192", "any") && has("512x512", "any") && has("512x512", "maskable"));
    let shortcuts: Vec<&str> = manifest["shortcuts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["url"].as_str().unwrap())
        .collect();
    assert_eq!(shortcuts, ["/pos", "/display/main", "/dashboard"]);
}

#[tokio::test]
async fn the_service_worker_and_icons_are_served_in_all_in_one_mode() {
    let client = Client::new("pwa-assets", true).await;
    let worker = client.get("/sw.js").await;
    assert_eq!(worker.status, StatusCode::OK);
    assert_eq!(worker.headers["service-worker-allowed"], "/");
    assert!(worker.text().contains("addEventListener('fetch'"));
    assert_eq!(client.get("/offline.html").await.status, StatusCode::OK);
    for icon in [
        "icon-192.png",
        "icon-512.png",
        "icon-maskable-512.png",
        "apple-touch-icon.png",
    ] {
        let res = client.get(&format!("/static/{icon}")).await;
        assert_eq!(res.status, StatusCode::OK, "{icon}");
        assert_eq!(&res.body[1..4], b"PNG", "{icon}");
    }
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
async fn a_customer_order_is_refused_when_payments_cannot_be_verified() {
    let mut client = Client::new("selfpay", false).await;
    client.login().await;
    let product = client.first_product().await;

    // No verify API is set up: a plain QR would be paid but never noticed, so no order is made.
    let refused = client
        .post(
            "/api/m/main/orders",
            json!({ "items": [{ "product_id": product, "qty": 1 }] }),
        )
        .await;
    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        refused.json()["error"]
            .as_str()
            .unwrap()
            .contains("ชำระเงิน")
    );
    let active = client.get("/api/orders/active").await.json();
    assert_eq!(active.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn the_bot_status_needs_a_login_and_says_when_checking_is_not_set_up() {
    let mut client = Client::new("botstatus", false).await;
    assert_eq!(
        client.get("/api/settings/bot").await.status,
        StatusCode::UNAUTHORIZED
    );
    client.login().await;
    let status = client.get("/api/settings/bot").await;
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(status.json()["state"], "not_configured");
    assert_eq!(status.json()["ok"], false);
}

#[tokio::test]
async fn a_payment_verified_by_the_bot_pays_the_order_and_keeps_its_slip() {
    let mut client = Client::new("verified-paid", false).await;
    client.login().await;
    let product = client.first_product().await;
    let order = client
        .post(
            "/api/orders",
            json!({ "method": "promptpay", "items": [{ "product_id": product, "qty": 1 }] }),
        )
        .await
        .json();
    let id = order["id"].as_i64().unwrap();

    // What the payment poller does when the API reports the money arrived.
    let changed = orders::mark_paid(
        &client.state,
        id,
        Some(orders::ExternalPayment {
            id: "pp_test-1".into(),
            bank: Some("kbank".into()),
            payer_name: Some("ทดสอบ".into()),
        }),
    )
    .await
    .ok()
    .unwrap();
    assert!(changed, "the order was paid");
    let active = client.get("/api/orders/active").await.json();
    assert_eq!(active[0]["status"], "paid");
    assert_eq!(active[0]["queue_label"], "A001");

    let today = (chrono::Utc::now() + chrono::Duration::hours(7))
        .format("%Y-%m-%d")
        .to_string();
    let slips = client
        .get(&format!("/api/dashboard?period=day&date={today}"))
        .await
        .json()["slips"]
        .clone();
    assert_eq!(slips[0]["verified"], true);
    assert_eq!(slips[0]["bank"], "kbank");
    assert_eq!(slips[0]["reference"], "pp_test-1");

    // The same payment arriving twice (the poller can see it again) changes nothing.
    let again = orders::mark_paid(&client.state, id, None)
        .await
        .ok()
        .unwrap();
    assert!(!again, "already paid");
}

#[tokio::test]
async fn the_bot_list_for_choosing_needs_a_login_and_explains_when_it_cannot_be_fetched() {
    let mut client = Client::new("bot-list", false).await;
    assert_eq!(
        client.get("/api/settings/routes").await.status,
        StatusCode::UNAUTHORIZED
    );
    client.login().await;
    let list = client.get("/api/settings/routes").await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.json()["ok"], false);
    assert!(list.json()["message"].as_str().unwrap().contains("ไม่ครบ"));
}

#[tokio::test]
async fn resetting_sales_clears_orders_and_restarts_the_queue_but_keeps_the_menu() {
    let mut client = Client::new("reset-sales", false).await;
    let body = json!({ "confirm": "reset-sales", "password": ADMIN_PASSWORD });
    assert_eq!(
        client
            .post("/api/settings/reset-sales", body.clone())
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    client.login().await;
    let product = client.first_product().await;
    let first = client.cash_order(product, None).await.json();
    assert_eq!(first["queue_label"], "A001");
    client.cash_order(product, None).await;

    // Without the confirmation word nothing is deleted.
    let refused = client
        .post("/api/settings/reset-sales", json!({ "confirm": "yes" }))
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        client
            .get("/api/orders/active")
            .await
            .json()
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let done = client.post("/api/settings/reset-sales", body).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    assert_eq!(done.json()["orders"], 2);
    assert!(
        client
            .get("/api/orders/active")
            .await
            .json()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        client
            .get(&format!("/api/q/{}", first["token"].as_str().unwrap()))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // The menu is untouched and the next sale is queue number one again.
    assert!(
        !client
            .get("/api/products")
            .await
            .json()
            .as_array()
            .unwrap()
            .is_empty()
    );
    let next = client.cash_order(product, None).await.json();
    assert_eq!(next["queue_label"], "A001");
}

#[tokio::test]
async fn a_promptpay_payment_leaves_a_slip_that_outlives_the_orders() {
    let mut client = Client::new("slips", false).await;
    client.login().await;
    let product = client.first_product().await;
    let today = (chrono::Utc::now() + chrono::Duration::hours(7))
        .format("%Y-%m-%d")
        .to_string();
    let dashboard = format!("/api/dashboard?period=day&date={today}");

    let order = client
        .post(
            "/api/orders",
            json!({ "method": "promptpay", "items": [{ "product_id": product, "qty": 2 }] }),
        )
        .await;
    assert_eq!(order.status, StatusCode::OK, "{}", order.text());
    let id = order.json()["id"].as_i64().unwrap();
    // Cash sales leave no slip; only the PromptPay one does, and only once it is paid.
    client.cash_order(product, None).await;
    assert!(
        client.get(&dashboard).await.json()["slips"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let paid = client
        .post(&format!("/api/orders/{id}/confirm"), json!({}))
        .await;
    assert_eq!(paid.status, StatusCode::OK, "{}", paid.text());

    let slips = client.get(&dashboard).await.json()["slips"].clone();
    let slips = slips.as_array().unwrap();
    assert_eq!(slips.len(), 1);
    assert_eq!(slips[0]["order_id"], id);
    assert_eq!(
        slips[0]["verified"], false,
        "confirmed by hand, not by the bot"
    );
    assert_eq!(slips[0]["items"][0]["qty"], 2);
    assert_eq!(slips[0]["queue_label"], "A002");

    // Starting over deletes the orders but the slip stays.
    let reset = client
        .post(
            "/api/settings/reset-sales",
            json!({ "confirm": "reset-sales", "password": ADMIN_PASSWORD }),
        )
        .await;
    assert_eq!(reset.status, StatusCode::OK);
    assert_eq!(
        client.get(&dashboard).await.json()["slips"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// An order waiting for a PromptPay payment; `expires` is an SQLite time modifier ("+2 minutes").
async fn open_qr(state: &AppState, token: &str, pay_state: &str, expires: Option<&str>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO orders (shop_id, token, queue_date, status, method, source, total_satang, \
         pay_state, pay_expires_at) VALUES (?, ?, date('now'), 'pending_payment', 'promptpay', \
         'pos', 5000, ?, CASE WHEN ? IS NULL THEN NULL ELSE datetime('now', ?) END) RETURNING id",
    )
    .bind(state.shop.id)
    .bind(token)
    .bind(pay_state)
    .bind(expires)
    .bind(expires)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn only_one_customer_qr_is_open_at_a_time() {
    let client = Client::new("qr-lock", false).await;
    let state = &client.state;
    assert!(
        orders::ensure_qr_free(state, None).await.is_ok(),
        "nothing waiting"
    );

    let id = open_qr(state, "t-live", "pending", Some("+2 minutes")).await;
    assert!(
        orders::ensure_qr_free(state, None).await.is_err(),
        "a live QR blocks others"
    );
    assert!(
        orders::ensure_qr_free(state, Some(id)).await.is_ok(),
        "its own order is not blocked by itself"
    );

    sqlx::query("UPDATE orders SET pay_expires_at = datetime('now', '-1 minutes') WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .unwrap();
    assert!(
        orders::ensure_qr_free(state, None).await.is_ok(),
        "an expired QR frees the line"
    );

    let queued = open_qr(state, "t-queued", "queued", None).await;
    assert!(
        orders::ensure_qr_free(state, None).await.is_err(),
        "a payment waiting its turn counts"
    );
    sqlx::query("UPDATE orders SET status = 'cancelled' WHERE id = ?")
        .bind(queued)
        .execute(&state.pool)
        .await
        .unwrap();
    assert!(
        orders::ensure_qr_free(state, None).await.is_ok(),
        "cancelling releases it"
    );
}

#[tokio::test]
async fn the_menu_is_a_tree_of_groups_with_their_own_colours() {
    let mut client = Client::new("menu-tree", false).await;
    client.login().await;
    let products = || async { client.get("/api/products").await.json() };

    // Every seeded row got a colour, no two the same at the top level.
    let all = products().await;
    let colors: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["color"].as_str().expect("colour"))
        .collect();
    let unique: std::collections::HashSet<_> = colors.iter().collect();
    assert_eq!(unique.len(), colors.len(), "{colors:?}");

    // A group has no price; items go inside it and get colours of their own.
    let group = client
        .post(
            "/api/products",
            json!({ "name": "ชาไทย", "category": "ชา", "price_satang": 0, "is_group": true }),
        )
        .await;
    assert_eq!(group.status, StatusCode::OK, "{}", group.text());
    let group_id = group.json()["id"].as_i64().unwrap();
    let mut kids = Vec::new();
    for name in ["เย็น", "หวานน้อย"] {
        let kid = client
            .post(
                "/api/products",
                json!({ "name": name, "category": "ignored", "price_satang": 4500, "parent_id": group_id }),
            )
            .await;
        assert_eq!(kid.status, StatusCode::OK, "{}", kid.text());
        kids.push(kid.json()["id"].as_i64().unwrap());
    }
    let listed = products().await;
    let row = |id: i64| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(row(group_id)["is_group"], true);
    assert_eq!(row(group_id)["price_satang"], 0);
    assert_eq!(row(kids[0])["parent_id"], group_id);
    assert_eq!(
        row(kids[0])["category"],
        "",
        "inside a group the category is not used"
    );
    assert_ne!(row(kids[0])["color"], row(kids[1])["color"]);

    // The same colour twice side by side is refused; the same colour in another group is fine.
    let taken = row(kids[0])["color"].clone();
    let dup = client
        .put(
            &format!("/api/products/{}", kids[1]),
            json!({ "name": "หวานน้อย", "category": "", "price_satang": 4500, "parent_id": group_id, "color": taken }),
        )
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT, "{}", dup.text());
    assert!(dup.json()["error"].as_str().unwrap().contains("เย็น"));
    let bad = client
        .post(
            "/api/products",
            json!({ "name": "x", "category": "", "price_satang": 100, "color": "red" }),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // Only groups can hold things, a group cannot go under itself, and groups cannot be ordered.
    let under_item = client
        .put(
            &format!("/api/products/{}", kids[1]),
            json!({ "name": "หวานน้อย", "category": "", "price_satang": 4500, "parent_id": kids[0] }),
        )
        .await;
    assert_eq!(under_item.status, StatusCode::BAD_REQUEST);
    let loop_ = client
        .put(
            &format!("/api/products/{group_id}"),
            json!({ "name": "ชาไทย", "category": "ชา", "price_satang": 0, "parent_id": group_id, "is_group": true }),
        )
        .await;
    assert_eq!(loop_.status, StatusCode::BAD_REQUEST);
    let order = client.cash_order(group_id, None).await;
    assert_eq!(
        order.status,
        StatusCode::BAD_REQUEST,
        "a group cannot be sold"
    );
    assert_eq!(
        client.cash_order(kids[0], None).await.status,
        StatusCode::OK
    );

    // Deleting a group takes what is inside it along.
    let gone = client
        .send(
            Method::DELETE,
            &format!("/api/products/{group_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(gone.status, StatusCode::OK);
    let left = products().await;
    assert!(
        left.as_array()
            .unwrap()
            .iter()
            .all(|p| p["id"] != kids[0] && p["id"] != group_id)
    );
}

#[tokio::test]
async fn menu_rows_can_be_put_in_any_order_one_group_at_a_time() {
    let mut client = Client::new("menu-order", false).await;
    client.login().await;
    let make = |name: &'static str, parent: Option<i64>| json!({ "name": name, "category": "", "price_satang": 1000, "parent_id": parent });
    for name in ["ก", "ข", "ค"] {
        let made = client.post("/api/products", make(name, None)).await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.text());
    }
    let names = || async {
        client
            .get("/api/products")
            .await
            .json()
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let before = names().await;
    let top: Vec<i64> = client
        .get("/api/products")
        .await
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();

    // Reverse the whole top level.
    let reversed: Vec<i64> = top.iter().rev().copied().collect();
    let done = client
        .put("/api/products/order", json!({ "ids": reversed }))
        .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    let after = names().await;
    assert_eq!(after, before.iter().rev().cloned().collect::<Vec<_>>());

    // A list that is missing a row, repeats one or names a stranger is refused, nothing changes.
    for bad in [
        reversed[1..].to_vec(),
        vec![reversed[0]; reversed.len()],
        [reversed.clone(), vec![9999]].concat(),
    ] {
        let refused = client
            .put("/api/products/order", json!({ "ids": bad }))
            .await;
        assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text());
    }
    assert_eq!(names().await, after);
    assert_eq!(
        client
            .put("/api/products/order", json!({ "ids": [] }))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn an_older_page_that_leaves_out_the_group_does_not_pull_an_item_out_of_it() {
    let mut client = Client::new("menu-stale-page", false).await;
    client.login().await;
    let group = client
        .post(
            "/api/products",
            json!({ "name": "ชา", "category": "", "price_satang": 0, "is_group": true }),
        )
        .await
        .json()["id"]
        .as_i64()
        .unwrap();
    let item = client
        .post(
            "/api/products",
            json!({ "name": "เย็น", "category": "", "price_satang": 4500, "parent_id": group }),
        )
        .await
        .json()["id"]
        .as_i64()
        .unwrap();

    // A page loaded before groups existed sends only name, category and price.
    let renamed = client
        .put(
            &format!("/api/products/{item}"),
            json!({ "name": "เย็นมาก", "category": "", "price_satang": 5000 }),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    let listed = client.get("/api/products").await.json();
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == item)
        .unwrap();
    assert_eq!(row["name"], "เย็นมาก");
    assert_eq!(row["price_satang"], 5000);
    assert_eq!(row["parent_id"], group, "still inside its group");

    // Saying "top level" on purpose still moves it out.
    let out = client
        .put(
            &format!("/api/products/{item}"),
            json!({ "name": "เย็นมาก", "category": "", "price_satang": 5000, "parent_id": null }),
        )
        .await;
    assert_eq!(out.status, StatusCode::OK, "{}", out.text());
    let listed = client.get("/api/products").await.json();
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == item)
        .unwrap();
    assert_eq!(row["parent_id"], Value::Null);
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

// ------------------------------------------------------------------- staff and audit

/// An admin client plus a second client logged in as a fresh cashier account.
async fn admin_and_cashier(name: &str) -> (Client, Client) {
    let mut admin = Client::new(name, false).await;
    admin.login().await;
    let made = admin
        .post(
            "/api/users",
            json!({ "username": "somchai", "password": "cashier-pass-1", "role": "cashier" }),
        )
        .await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.text());
    let mut cashier = admin.another_session();
    cashier.login_as("somchai", "cashier-pass-1").await;
    (admin, cashier)
}

async fn order_action(client: &Client, id: i64, what: &str) -> Reply {
    client
        .post(&format!("/api/orders/{id}/{what}"), json!({}))
        .await
}

fn today_bangkok() -> String {
    (chrono::Utc::now() + chrono::Duration::hours(7))
        .format("%Y-%m-%d")
        .to_string()
}

#[tokio::test]
async fn a_cashier_can_sell_but_not_reach_setup_takings_or_accounts() {
    let (admin, cashier) = admin_and_cashier("roles").await;
    let me = cashier.get("/api/me").await.json();
    assert_eq!(me["username"], "somchai");
    assert_eq!(me["role"], "cashier");
    assert_eq!(admin.get("/api/me").await.json()["role"], "admin");

    // What a till needs works.
    let product = cashier.first_product().await;
    assert_eq!(
        cashier.cash_order(product, None).await.status,
        StatusCode::OK
    );
    assert_eq!(
        cashier.get("/api/orders/active").await.status,
        StatusCode::OK
    );
    assert_eq!(
        cashier.get("/api/print/settings").await.status,
        StatusCode::OK
    );

    // What sets the shop up, shows the takings or manages people does not.
    let dashboard = format!("/api/dashboard?period=day&date={}", today_bangkok());
    let new_product = json!({ "name": "x", "category": "", "price_satang": 100 });
    let reset = json!({ "confirm": "reset-sales", "password": ADMIN_PASSWORD });
    for (what, status) in [
        ("dashboard", cashier.get(&dashboard).await.status),
        ("users", cashier.get("/api/users").await.status),
        ("bot status", cashier.get("/api/settings/bot").await.status),
        (
            "bill settings",
            cashier.get("/api/bill/settings").await.status,
        ),
        (
            "new product",
            cashier.post("/api/products", new_product).await.status,
        ),
        (
            "reset",
            cashier
                .post("/api/settings/reset-sales", reset)
                .await
                .status,
        ),
    ] {
        assert_eq!(status, StatusCode::FORBIDDEN, "{what}");
    }
    // The admin can, and a cashier only learns whether automatic checking is on.
    assert_eq!(admin.get(&dashboard).await.status, StatusCode::OK);
    let settings = cashier.get("/api/settings").await.json();
    let keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["verify_active"]);
    assert!(
        admin
            .get("/api/settings")
            .await
            .json()
            .get("base_url")
            .is_some()
    );
}

#[tokio::test]
async fn account_rules_keep_the_shop_manageable() {
    let (admin, cashier) = admin_and_cashier("account-rules").await;
    let users = admin.get("/api/users").await.json();
    let id_of = |name: &str| {
        users
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["username"] == name)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    };
    let (admin_id, cashier_id) = (id_of("admin"), id_of("somchai"));

    let dup = json!({ "username": "somchai", "password": "another-pass-1", "role": "cashier" });
    assert_eq!(
        admin.post("/api/users", dup).await.status,
        StatusCode::CONFLICT
    );
    for bad in [
        json!({ "username": "ab", "password": "long-enough-1", "role": "cashier" }),
        json!({ "username": "has space", "password": "long-enough-1", "role": "cashier" }),
        json!({ "username": "okname", "password": "short", "role": "cashier" }),
        json!({ "username": "okname", "password": "long-enough-1", "role": "boss" }),
    ] {
        assert_eq!(
            admin.post("/api/users", bad).await.status,
            StatusCode::BAD_REQUEST
        );
    }

    // Nobody can lock the shop out of its own setup.
    let delete_self = admin
        .send(
            Method::DELETE,
            &format!("/api/users/{admin_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(delete_self.status, StatusCode::BAD_REQUEST);
    let demote = admin
        .put(
            &format!("/api/users/{admin_id}"),
            json!({ "role": "cashier" }),
        )
        .await;
    assert_eq!(demote.status, StatusCode::BAD_REQUEST, "{}", demote.text());

    // Switching an account off ends its session at once and keeps it from logging in again.
    assert_eq!(cashier.get("/api/me").await.status, StatusCode::OK);
    let off = admin
        .put(
            &format!("/api/users/{cashier_id}"),
            json!({ "active": false }),
        )
        .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.text());
    assert_eq!(
        cashier.get("/api/me").await.status,
        StatusCode::UNAUTHORIZED
    );
    let mut again = admin.another_session();
    assert!(
        !again.try_login("somchai", "cashier-pass-1").await,
        "switched off"
    );

    // Back on, with a new password: only the new one works.
    let back = json!({ "active": true, "password": "brand-new-pass-2" });
    admin.put(&format!("/api/users/{cashier_id}"), back).await;
    assert!(!again.try_login("somchai", "cashier-pass-1").await);
    assert!(again.try_login("somchai", "brand-new-pass-2").await);
}

#[tokio::test]
async fn manual_confirms_and_cancellations_are_recorded_with_who_did_them() {
    let (admin, cashier) = admin_and_cashier("audit").await;
    let product = cashier.first_product().await;
    let promptpay =
        || json!({ "method": "promptpay", "items": [{ "product_id": product, "qty": 1 }] });
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(
            cashier.post("/api/orders", promptpay()).await.json()["id"]
                .as_i64()
                .unwrap(),
        );
    }
    let act = |id: i64, what: &str| {
        let path = format!("/api/orders/{id}/{what}");
        let cashier = &cashier;
        async move { cashier.post(&path, json!({})).await }
    };
    assert_eq!(act(ids[0], "confirm").await.status, StatusCode::OK);
    assert_eq!(act(ids[1], "confirm").await.status, StatusCode::OK);
    // One order cancelled before it was paid, one after.
    assert_eq!(act(ids[2], "cancel").await.status, StatusCode::OK);
    assert_eq!(act(ids[1], "cancel").await.status, StatusCode::OK);
    // Confirming an order that is already paid is not a second record.
    act(ids[0], "confirm").await;

    let dashboard = format!("/api/dashboard?period=day&date={}", today_bangkok());
    let audit = admin.get(&dashboard).await.json()["audit"].clone();
    assert_eq!(audit["counts"]["confirm_manual"], 2);
    assert_eq!(audit["counts"]["cancel"], 1);
    assert_eq!(audit["counts"]["cancel_paid"], 1);
    let rows = audit["rows"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|r| r["action"] == "confirm_manual")
        .unwrap();
    assert_eq!(row["actor"], "somchai");
    assert_eq!(row["role"], "cashier");
    assert!(
        row["detail"].as_str().unwrap().contains('฿'),
        "the amount is in the line"
    );

    // The reset and the account changes are in the record too, and the record survives the reset.
    admin
        .post(
            "/api/settings/reset-sales",
            json!({ "confirm": "reset-sales", "password": ADMIN_PASSWORD }),
        )
        .await;
    let after = admin.get(&dashboard).await.json()["audit"].clone();
    assert_eq!(after["counts"]["reset_sales"], 1);
    assert_eq!(after["counts"]["staff_added"], 1);
    assert_eq!(after["counts"]["confirm_manual"], 2);
}

// ----------------------------------------------------------- close the day, reset, backup

#[tokio::test]
async fn the_reset_needs_the_admins_password_and_stops_guessing() {
    let mut client = Client::new("reset-password", false).await;
    client.login().await;
    let product = client.first_product().await;
    client.cash_order(product, None).await;
    let active = || async {
        client
            .get("/api/orders/active")
            .await
            .json()
            .as_array()
            .unwrap()
            .len()
    };

    for wrong in [
        json!({ "confirm": "reset-sales" }),
        json!({ "confirm": "reset-sales", "password": "nope" }),
    ] {
        let refused = client.post("/api/settings/reset-sales", wrong).await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{}",
            refused.text()
        );
    }
    assert_eq!(active().await, 1, "nothing was deleted");

    // Guessing is limited: after five misses even the right password waits.
    for _ in 0..3 {
        client
            .post(
                "/api/settings/reset-sales",
                json!({ "confirm": "reset-sales", "password": "guess" }),
            )
            .await;
    }
    let locked = client
        .post(
            "/api/settings/reset-sales",
            json!({ "confirm": "reset-sales", "password": ADMIN_PASSWORD }),
        )
        .await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(active().await, 1);
}

#[tokio::test]
async fn closing_the_day_freezes_the_takings_and_the_cash_difference() {
    let (admin, cashier) = admin_and_cashier("close-day").await;
    let product = cashier.first_product().await;
    let day = today_bangkok();

    // 2 cash sales, one QR payment confirmed by hand, one confirmed by the bank check.
    cashier.cash_order(product, None).await;
    cashier.cash_order(product, None).await;
    let promptpay =
        json!({ "method": "promptpay", "items": [{ "product_id": product, "qty": 1 }] });
    let by_hand = cashier.post("/api/orders", promptpay.clone()).await.json()["id"]
        .as_i64()
        .unwrap();
    order_action(&cashier, by_hand, "confirm").await;
    let by_bank = cashier.post("/api/orders", promptpay).await.json()["id"]
        .as_i64()
        .unwrap();
    orders::mark_paid(
        &admin.state,
        by_bank,
        Some(orders::ExternalPayment {
            id: "pp_close-1".into(),
            bank: Some("scb".into()),
            payer_name: None,
        }),
    )
    .await
    .ok()
    .unwrap();

    let preview = admin
        .get(&format!("/api/day/preview?day={day}"))
        .await
        .json();
    assert_eq!(preview["orders"], 4);
    assert_eq!(preview["manual_confirms"], 1);
    assert_eq!(preview["verified_payments"], 1);
    assert_eq!(preview["closed"], Value::Null);
    let cash = preview["cash_satang"].as_i64().unwrap();
    let revenue = preview["revenue_satang"].as_i64().unwrap();
    assert!(
        cash > 0 && revenue > cash,
        "cash {cash} of revenue {revenue}"
    );
    assert_eq!(preview["qr_satang"].as_i64().unwrap() + cash, revenue);

    // 5 baht short in the drawer.
    let closed = admin
        .post(
            "/api/day/close",
            json!({ "cash_counted_satang": cash - 500, "note": "ทอนผิด" }),
        )
        .await;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.text());
    assert_eq!(closed.json()["difference_satang"], -500);

    let after = admin
        .get(&format!("/api/day/preview?day={day}"))
        .await
        .json();
    assert_eq!(after["closed"]["closed_by"], "admin");
    assert_eq!(after["closed"]["difference_satang"], -500);
    let history = admin.get("/api/day/closes").await.json();
    assert_eq!(history[0]["day"], day);
    assert_eq!(history[0]["revenue_satang"], revenue);
    assert_eq!(history[0]["note"], "ทอนผิด");

    // A cashier cannot see or make closings; the future and nonsense are refused; it is recorded.
    assert_eq!(
        cashier.get("/api/day/closes").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        cashier.post("/api/day/close", json!({})).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        admin
            .post("/api/day/close", json!({ "day": "2999-01-01" }))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        admin
            .post("/api/day/close", json!({ "day": "not-a-day" }))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        admin
            .post("/api/day/close", json!({ "cash_counted_satang": -1 }))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    let audit = admin
        .get(&format!("/api/dashboard?period=day&date={day}"))
        .await
        .json()["audit"]
        .clone();
    assert_eq!(audit["counts"]["close_day"], 1);

    // The sales reset leaves the closing report in place.
    admin
        .post(
            "/api/settings/reset-sales",
            json!({ "confirm": "reset-sales", "password": ADMIN_PASSWORD }),
        )
        .await;
    assert_eq!(
        admin
            .get("/api/day/closes")
            .await
            .json()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_backup_is_a_whole_working_copy_only_an_admin_can_take() {
    let (admin, cashier) = admin_and_cashier("backup").await;
    let product = admin.first_product().await;
    admin.cash_order(product, None).await;
    admin.cash_order(product, None).await;

    assert_eq!(
        cashier.post("/api/backup", json!({})).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        cashier.get("/api/backup/files").await.status,
        StatusCode::FORBIDDEN
    );
    let taken = admin.post("/api/backup", json!({})).await;
    assert_eq!(taken.status, StatusCode::OK);
    assert!(
        taken
            .headers
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("posmini-"),
        "named like a backup"
    );
    backup::looks_like_sqlite(&taken.body).expect("a SQLite file");

    // The copy opens as a database that has the sales in it.
    let path =
        std::env::temp_dir().join(format!("posmini-restore-check-{}.db", std::process::id()));
    std::fs::write(&path, &taken.body).unwrap();
    let copy = db::connect(&format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    let orders: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orders")
        .fetch_one(&copy)
        .await
        .unwrap();
    assert_eq!(orders, 2);
    copy.close().await;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }

    // The kept copy is listed and downloadable by name; nothing else on the disk is.
    let listed = admin.get("/api/backup/files").await.json();
    let name = listed["files"][0]["name"].as_str().unwrap().to_string();
    let again = admin.get(&format!("/api/backup/files/{name}")).await;
    assert_eq!(again.status, StatusCode::OK);
    backup::looks_like_sqlite(&again.body).unwrap();
    for hostile in [
        "..%2F..%2Fetc%2Fpasswd",
        "posmini-x.txt",
        "secret.db",
        "posmini-..%2Fx.db",
    ] {
        let refused = admin.get(&format!("/api/backup/files/{hostile}")).await;
        assert!(
            matches!(
                refused.status,
                StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
            ),
            "{hostile} -> {}",
            refused.status
        );
    }
    let audit = admin
        .get(&format!(
            "/api/dashboard?period=day&date={}",
            today_bangkok()
        ))
        .await
        .json()["audit"]
        .clone();
    assert_eq!(audit["counts"]["backup"], 2, "both downloads are recorded");
}

// ----------------------------------------------- pretend servers, till warning, sold out

/// A pretend verify API: lists one bot whose status the test can change, and counts the questions.
struct MockVerify {
    base: String,
    bot_status: std::sync::Arc<std::sync::Mutex<String>>,
    broken: std::sync::Arc<std::sync::atomic::AtomicBool>,
    asked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

async fn mock_verify() -> MockVerify {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    #[derive(Clone)]
    struct Shared {
        status: Arc<Mutex<String>>,
        broken: Arc<AtomicBool>,
        asked: Arc<AtomicUsize>,
    }
    let shared = Shared {
        status: Arc::new(Mutex::new("online".to_string())),
        broken: Arc::new(AtomicBool::new(false)),
        asked: Arc::new(AtomicUsize::new(0)),
    };
    async fn bots(
        axum::extract::State(s): axum::extract::State<Shared>,
    ) -> (StatusCode, axum::Json<Value>) {
        s.asked.fetch_add(1, Ordering::SeqCst);
        if s.broken.load(Ordering::SeqCst) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": { "message": "boom" } })),
            );
        }
        let status = s.status.lock().unwrap().clone();
        (
            StatusCode::OK,
            axum::Json(
                json!({ "data": [{ "id": "bot_1", "name": "nid", "enabled": true, "status": status }] }),
            ),
        )
    }
    let app = Router::new()
        .route("/v1/bots", axum::routing::get(bots))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    MockVerify {
        base,
        bot_status: shared.status,
        broken: shared.broken,
        asked: shared.asked,
    }
}

fn pay_settings_for(base: &str) -> PaySettings {
    PaySettings {
        base_url: base.to_string(),
        api_key: "test-key".into(),
        promptpay_id: "0812345678".into(),
        ..PaySettings::default()
    }
}

#[tokio::test]
async fn the_till_warning_follows_what_the_bot_is_doing() {
    use std::sync::atomic::Ordering;
    let mock = mock_verify().await;
    let mut cashier_side = Client::new("pay-health", false).await;
    cashier_side.login().await;

    // A shop that never set automatic checking up has nothing to warn about.
    let manual = cashier_side.get("/api/pay/health").await.json();
    assert_eq!(manual["mode"], "manual");

    let settings = pay_settings_for(&mock.base);
    let switch_on = || {
        cashier_side
            .state
            .set_pay(PayRuntime::build(pay_settings_for(&mock.base)).unwrap())
    };
    let _ = settings;
    switch_on();
    let ok = cashier_side.get("/api/pay/health").await.json();
    assert_eq!(
        (ok["mode"].as_str(), ok["ok"].as_bool()),
        (Some("auto"), Some(true))
    );

    // Many tills asking within a few seconds cost the verify API one question, not one each.
    for _ in 0..5 {
        cashier_side.get("/api/pay/health").await;
    }
    assert_eq!(mock.asked.load(Ordering::SeqCst), 1);

    // The bot goes down: new settings clear the remembered answer and the warning shows.
    *mock.bot_status.lock().unwrap() = "offline".into();
    switch_on();
    let down = cashier_side.get("/api/pay/health").await.json();
    assert_eq!(
        (
            down["mode"].as_str(),
            down["ok"].as_bool(),
            down["state"].as_str()
        ),
        (Some("auto"), Some(false), Some("offline"))
    );
    assert!(down["message"].as_str().unwrap().contains("ไม่ทำงาน"));

    // The API itself failing is a warning too, not a silent pass.
    mock.broken.store(true, Ordering::SeqCst);
    switch_on();
    let broken = cashier_side.get("/api/pay/health").await.json();
    assert_eq!(
        (broken["ok"].as_bool(), broken["state"].as_str()),
        (Some(false), Some("error"))
    );

    // Needs a login like everything on the till.
    let nobody = cashier_side.another_session();
    assert_eq!(
        nobody.get("/api/pay/health").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_sold_out_item_cannot_be_ordered_until_it_is_back() {
    let (admin, cashier) = admin_and_cashier("sold-out").await;
    let product = cashier.first_product().await;
    let name = cashier.get("/api/products").await.json()[0]["name"]
        .as_str()
        .unwrap()
        .to_string();
    let toggle = |on: bool| {
        let path = format!("/api/products/{product}/sold-out");
        let cashier = &cashier;
        async move { cashier.put(&path, json!({ "sold_out": on })).await }
    };

    assert_eq!(toggle(true).await.status, StatusCode::OK);
    let listed = cashier.get("/api/products").await.json();
    assert_eq!(listed[0]["sold_out"], true);
    let refused = cashier.cash_order(product, None).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text());
    assert!(refused.json()["error"].as_str().unwrap().contains(&name));
    assert!(
        cashier
            .get("/api/orders/active")
            .await
            .json()
            .as_array()
            .unwrap()
            .is_empty(),
        "no half-made order is left behind"
    );

    assert_eq!(toggle(false).await.status, StatusCode::OK);
    assert_eq!(
        cashier.cash_order(product, None).await.status,
        StatusCode::OK
    );

    // Only an item can be sold out, not a group; unknown ids are not found; it needs a login.
    let group = admin
        .post(
            "/api/products",
            json!({ "name": "กลุ่ม", "category": "", "price_satang": 0, "is_group": true }),
        )
        .await
        .json()["id"]
        .as_i64()
        .unwrap();
    let on_group = cashier
        .put(
            &format!("/api/products/{group}/sold-out"),
            json!({ "sold_out": true }),
        )
        .await;
    assert_eq!(on_group.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        cashier
            .put("/api/products/99999/sold-out", json!({ "sold_out": true }))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let nobody = admin.another_session();
    assert_eq!(
        nobody
            .put(
                &format!("/api/products/{product}/sold-out"),
                json!({ "sold_out": true })
            )
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    // Both switches are in the record.
    let audit = admin
        .get(&format!(
            "/api/dashboard?period=day&date={}",
            today_bangkok()
        ))
        .await
        .json()["audit"]
        .clone();
    assert_eq!(audit["counts"]["sold_out"], 2);
}
