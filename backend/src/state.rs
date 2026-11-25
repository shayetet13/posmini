use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::{
    bill::BillSettings, config::Config, db::Shop, events::Event, payment::PaymentClient,
    printing::PrintSettings, settings::PaySettings,
};

/// Payment settings plus the verify-API client built from them (if complete).
/// Held by a customer-display event stream for as long as that display is connected.
pub struct DisplayGuard(AppState);

impl Drop for DisplayGuard {
    fn drop(&mut self) {
        let count = self
            .0
            .displays
            .fetch_sub(1, Ordering::Relaxed)
            .saturating_sub(1);
        self.0
            .publish(Event::new("displays", json!({ "count": count })));
    }
}

pub struct PayRuntime {
    pub settings: PaySettings,
    pub client: Option<PaymentClient>,
}

impl PayRuntime {
    pub fn build(settings: PaySettings) -> anyhow::Result<Self> {
        let client = PaymentClient::from_settings(&settings)?;
        Ok(Self { settings, client })
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub shop: Arc<Shop>,
    pub hub: broadcast::Sender<Event>,
    pay: Arc<RwLock<Arc<PayRuntime>>>,
    print: Arc<RwLock<Arc<PrintSettings>>>,
    bill: Arc<RwLock<Arc<BillSettings>>>,
    print_lock: Arc<tokio::sync::Mutex<()>>,
    qr_lock: Arc<tokio::sync::Mutex<()>>,
    print_wake: Arc<tokio::sync::Notify>,
    station_seen: Arc<Mutex<Option<Instant>>>,
    cart: Arc<Mutex<Option<Value>>>,
    displays: Arc<AtomicUsize>,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    limits: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
/// A print station that has not asked for work for this long counts as offline.
const STATION_ONLINE: Duration = Duration::from_secs(20);
const LIMIT_MAP_CAP: usize = 5_000;

impl AppState {
    pub fn new(
        pool: SqlitePool,
        cfg: Config,
        shop: Shop,
        pay: PayRuntime,
        print: PrintSettings,
        bill: BillSettings,
    ) -> Self {
        let (hub, _) = broadcast::channel(256);
        Self {
            pool,
            cfg: Arc::new(cfg),
            shop: Arc::new(shop),
            hub,
            pay: Arc::new(RwLock::new(Arc::new(pay))),
            print: Arc::new(RwLock::new(Arc::new(print))),
            bill: Arc::new(RwLock::new(Arc::new(bill))),
            print_lock: Arc::default(),
            qr_lock: Arc::default(),
            print_wake: Arc::default(),
            station_seen: Arc::default(),
            cart: Arc::default(),
            displays: Arc::default(),
            sessions: Arc::default(),
            limits: Arc::default(),
        }
    }

    /// Current payment settings/client; settings can change while the server runs.
    pub fn pay(&self) -> Arc<PayRuntime> {
        let guard = self.pay.read().unwrap_or_else(|e| e.into_inner());
        Arc::clone(&guard)
    }

    pub fn set_pay(&self, runtime: PayRuntime) {
        let mut guard = self.pay.write().unwrap_or_else(|e| e.into_inner());
        *guard = Arc::new(runtime);
    }

    /// Current printer settings; they can change while the server runs.
    pub fn print(&self) -> Arc<PrintSettings> {
        let guard = self.print.read().unwrap_or_else(|e| e.into_inner());
        Arc::clone(&guard)
    }

    pub fn set_print(&self, settings: PrintSettings) {
        let mut guard = self.print.write().unwrap_or_else(|e| e.into_inner());
        *guard = Arc::new(settings);
    }

    /// Wording of tickets/receipts and the queue-number format; editable while running.
    pub fn bill(&self) -> Arc<BillSettings> {
        let guard = self.bill.read().unwrap_or_else(|e| e.into_inner());
        Arc::clone(&guard)
    }

    pub fn set_bill(&self, settings: BillSettings) {
        let mut guard = self.bill.write().unwrap_or_else(|e| e.into_inner());
        *guard = Arc::new(settings);
    }

    /// The cart the cashier last pushed, so a customer display opened later can show it.
    pub fn set_cart(&self, cart: Value) {
        let mut slot = self.cart.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some(cart);
    }

    pub fn cart(&self) -> Option<Value> {
        self.cart.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn displays(&self) -> usize {
        self.displays.load(Ordering::Relaxed)
    }

    /// Registers a connected customer display; the count drops again when the guard is dropped.
    pub fn display_connected(&self) -> DisplayGuard {
        let count = self.displays.fetch_add(1, Ordering::Relaxed) + 1;
        self.publish(Event::new("displays", json!({ "count": count })));
        DisplayGuard(self.clone())
    }

    /// Wakes the print worker when a job has been queued.
    pub fn print_wake(&self) -> &tokio::sync::Notify {
        &self.print_wake
    }

    /// A browser print station asked for work just now.
    pub fn note_station(&self) {
        let mut seen = self.station_seen.lock().unwrap_or_else(|e| e.into_inner());
        *seen = Some(Instant::now());
    }

    pub fn station_online(&self) -> bool {
        let seen = self.station_seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.is_some_and(|at| at.elapsed() < STATION_ONLINE)
    }

    pub fn print_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.print_lock
    }

    /// Held while a customer's QR is checked for, created and registered, so two customers who
    /// scan at the same moment cannot both pass the "one QR at a time" check.
    pub fn qr_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.qr_lock
    }

    pub fn publish(&self, event: Event) {
        let _ = self.hub.send(event);
    }

    pub fn create_session(&self) -> String {
        let sid = random_token();
        let mut map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.retain(|_, expires| *expires > now);
        map.insert(sid.clone(), now + SESSION_TTL);
        sid
    }

    pub fn session_valid(&self, sid: &str) -> bool {
        let map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        map.get(sid)
            .is_some_and(|expires| *expires > Instant::now())
    }

    pub fn drop_session(&self, sid: &str) {
        let mut map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(sid);
    }

    pub fn is_limited(&self, key: &str, max: usize, window: Duration) -> bool {
        let mut map = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        match map.get_mut(key) {
            Some(hits) => {
                hits.retain(|t| now.duration_since(*t) < window);
                hits.len() >= max
            }
            None => false,
        }
    }

    pub fn record_hit(&self, key: &str) {
        let mut map = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > LIMIT_MAP_CAP {
            map.clear();
        }
        map.entry(key.to_string()).or_default().push(Instant::now());
    }
}

pub fn random_token() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}
