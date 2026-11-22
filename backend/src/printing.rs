//! Thermal receipt printing: queue ticket after payment, receipt when the order is done.
//! The server prints (USB / Bluetooth printer attached to this PC), so an iPad or phone
//! only needs a browser. Bills are sent as a picture by default so Thai is always correct.

use std::{collections::HashMap, io::Write, path::Path};

use anyhow::{Context, Result, anyhow, bail};
#[cfg(windows)]
use printers::common::{base::job::PrinterJobOptions, converters::Converter};
use sqlx::SqlitePool;

use crate::{
    bill::{self, BillSettings, Doc, Facts, Line, Size},
    escpos::Builder,
    orders::OrderView,
    raster::{self, Bitmap},
    settings,
    state::AppState,
};

const BANGKOK_OFFSET_HOURS: i64 = 7;
const BUDDHIST_ERA_OFFSET: i32 = 543;
#[cfg(windows)]
const JOB_NAME: &str = "posmini";
/// Code pages commonly used for Thai on ESC/POS printers; the test page prints each one.
pub const CODEPAGE_CANDIDATES: [u8; 11] = [26, 255, 20, 21, 22, 23, 24, 25, 30, 41, 42];
const DEFAULT_CODEPAGE: u8 = 26;
/// Printable dots across the paper at 203 dpi.
const DOTS_58: usize = 384;
const DOTS_80: usize = 576;
pub const MIN_HEAD_DOTS: usize = 240;
pub const MAX_HEAD_DOTS: usize = 832;
pub const MAX_SHIFT_DOTS: i32 = 120;
pub const MAX_GAP_SECS: u64 = 30;
const DEFAULT_GAP_SECS: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// A printer installed in Windows (USB, or Bluetooth added as a printer).
    Printer,
    /// A serial port, e.g. the COM port Windows creates for a paired Bluetooth printer.
    Com,
    /// The browser on the shop's computer prints (Web Serial / WebUSB): for a server in the cloud.
    Web,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Printer => "printer",
            Self::Com => "com",
            Self::Web => "web",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "printer" => Some(Self::Printer),
            "com" => Some(Self::Com),
            "web" => Some(Self::Web),
            _ => None,
        }
    }
}

/// How a bill reaches the printer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Drawn as a picture: Thai is always right, any printer.
    Image,
    /// Plain text in the printer's own Thai code page (faster, needs the right code page).
    Text,
}

impl Style {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Text => "text",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "image" => Some(Self::Image),
            "text" => Some(Self::Text),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PrintSettings {
    pub mode: Mode,
    pub target: String,
    pub width: u8,
    pub codepage: u8,
    pub ticket: bool,
    pub cut: bool,
    pub style: Style,
    /// Shop copy of the order (big items, no prices) printed after the queue ticket.
    pub order_slip: bool,
    /// Printable width in dots; 0 = the usual 384 (58 mm) / 576 (80 mm).
    pub head_dots: usize,
    /// Moves the bill sideways in dots (positive = right) when it is not centred on the paper.
    pub shift: i32,
    /// Seconds to wait between one sheet and the next so each can be torn off.
    pub gap_secs: u64,
}

impl Default for PrintSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            target: String::new(),
            width: 58,
            codepage: DEFAULT_CODEPAGE,
            ticket: true,
            cut: true,
            style: Style::Image,
            order_slip: true,
            head_dots: 0,
            shift: 0,
            gap_secs: DEFAULT_GAP_SECS,
        }
    }
}

const KEYS: [&str; 11] = [
    "print_mode",
    "print_target",
    "print_width",
    "print_codepage",
    "print_ticket",
    "print_cut",
    "print_style",
    "print_slip",
    "print_dots",
    "print_shift",
    "print_gap",
];

impl PrintSettings {
    pub fn from_map(map: &HashMap<String, String>) -> Self {
        let base = Self::default();
        let text =
            |key: &str, default: &str| map.get(key).cloned().unwrap_or_else(|| default.into());
        let flag = |key: &str, default: bool| map.get(key).map_or(default, |v| v == "1");
        Self {
            mode: map
                .get(KEYS[0])
                .and_then(|v| Mode::parse(v))
                .unwrap_or(base.mode),
            target: text(KEYS[1], &base.target),
            width: map
                .get(KEYS[2])
                .and_then(|v| v.parse().ok())
                .filter(|w| matches!(w, 58 | 80))
                .unwrap_or(base.width),
            codepage: map
                .get(KEYS[3])
                .and_then(|v| v.parse().ok())
                .unwrap_or(base.codepage),
            ticket: flag(KEYS[4], base.ticket),
            cut: flag(KEYS[5], base.cut),
            style: map
                .get(KEYS[6])
                .and_then(|v| Style::parse(v))
                .unwrap_or(base.style),
            order_slip: flag(KEYS[7], base.order_slip),
            head_dots: map
                .get(KEYS[8])
                .and_then(|v| v.parse().ok())
                .filter(|d| *d == 0 || (MIN_HEAD_DOTS..=MAX_HEAD_DOTS).contains(d))
                .unwrap_or(base.head_dots),
            shift: map
                .get(KEYS[9])
                .and_then(|v| v.parse().ok())
                .filter(|s: &i32| s.abs() <= MAX_SHIFT_DOTS)
                .unwrap_or(base.shift),
            gap_secs: map
                .get(KEYS[10])
                .and_then(|v| v.parse().ok())
                .filter(|g| *g <= MAX_GAP_SECS)
                .unwrap_or(base.gap_secs),
        }
    }

    fn pairs(&self) -> Vec<(&'static str, String)> {
        let flag = |on: bool| if on { "1" } else { "0" }.to_string();
        vec![
            (KEYS[0], self.mode.as_str().to_string()),
            (KEYS[1], self.target.clone()),
            (KEYS[2], self.width.to_string()),
            (KEYS[3], self.codepage.to_string()),
            (KEYS[4], flag(self.ticket)),
            (KEYS[5], flag(self.cut)),
            (KEYS[6], self.style.as_str().to_string()),
            (KEYS[7], flag(self.order_slip)),
            (KEYS[8], self.head_dots.to_string()),
            (KEYS[9], self.shift.to_string()),
            (KEYS[10], self.gap_secs.to_string()),
        ]
    }

    /// Characters per line in the normal font.
    pub fn cols(&self) -> usize {
        if self.width == 80 { 48 } else { 32 }
    }

    /// Dots across the paper, for image printing.
    pub fn dots(&self) -> usize {
        match (self.head_dots, self.width) {
            (0, 80) => DOTS_80,
            (0, _) => DOTS_58,
            (custom, _) => custom,
        }
    }

    pub fn ready(&self) -> bool {
        match self.mode {
            Mode::Off => false,
            Mode::Web => true,
            Mode::Printer | Mode::Com => !self.target.is_empty(),
        }
    }
}

pub async fn load(pool: &SqlitePool, shop_id: i64) -> Result<PrintSettings> {
    Ok(PrintSettings::from_map(
        &settings::load_map(pool, shop_id).await?,
    ))
}

pub async fn save(pool: &SqlitePool, shop_id: i64, print: &PrintSettings) -> Result<()> {
    settings::save_pairs(pool, shop_id, &print.pairs()).await
}

// ------------------------------------------------------------------ layout

fn bangkok_now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() + chrono::Duration::hours(BANGKOK_OFFSET_HOURS)
}

/// e.g. `22/11/2569 15:29` (Buddhist era, Bangkok time).
fn now_stamp() -> String {
    stamp_of(bangkok_now())
}

/// A database time (`2026-11-22 08:29:00`, UTC) as the stamp on a bill, in Bangkok time.
pub fn stamp_of_db(utc: &str) -> String {
    chrono::NaiveDateTime::parse_from_str(utc, "%Y-%m-%d %H:%M:%S").map_or_else(
        |_| now_stamp(),
        |naive| stamp_of(naive.and_utc() + chrono::Duration::hours(BANGKOK_OFFSET_HOURS)),
    )
}

fn stamp_of(now: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::{Datelike, Timelike};
    format!(
        "{:02}/{:02}/{} {:02}:{:02}",
        now.day(),
        now.month(),
        now.year() + BUDDHIST_ERA_OFFSET,
        now.hour(),
        now.minute()
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The customer's bill: queue ticket and receipt in one.
    Ticket,
    /// Old name of the customer's bill; queued jobs and reprints may still use it.
    Receipt,
    /// Shop copy of the order.
    Slip,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ticket => "ticket",
            Self::Slip => "slip",
            Self::Receipt => "receipt",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ticket" => Some(Self::Ticket),
            "slip" => Some(Self::Slip),
            "receipt" => Some(Self::Receipt),
            _ => None,
        }
    }
}

/// What prints automatically when an order is paid, in paper order: the customer's bill
/// (queue ticket and receipt in one), then the shop's copy.
pub fn kinds_for(cfg: &PrintSettings) -> Vec<Kind> {
    let mut kinds = Vec::new();
    if cfg.ticket {
        kinds.push(Kind::Ticket);
    }
    if cfg.order_slip {
        kinds.push(Kind::Slip);
    }
    kinds
}

/// Which bill, laid out from the saved wording; shared by printing and the settings preview.
pub fn document(
    bill: &BillSettings,
    shop: &str,
    order: &OrderView,
    bank: Option<&str>,
    kind: Kind,
    stamp: &str,
) -> Doc {
    let facts = Facts { shop, bank, stamp };
    match kind {
        Kind::Ticket | Kind::Receipt => bill::ticket(bill, &facts, order),
        Kind::Slip => bill::order_slip(bill, &facts, order),
    }
}

fn finish(cfg: &PrintSettings, mut b: Builder) -> Vec<u8> {
    b.normal().feed(if cfg.cut { 3 } else { 5 });
    if cfg.cut {
        b.cut();
    }
    b.finish()
}

fn encode_text(cfg: &PrintSettings, doc: &Doc) -> Vec<u8> {
    let mut b = Builder::new(cfg.cols(), cfg.codepage);
    for line in doc {
        match line {
            Line::Text {
                text,
                size,
                bold,
                align,
            } => {
                let scale: u8 = match size {
                    Size::Normal => 1,
                    Size::Title | Size::Large => 2,
                    Size::Huge => 4,
                };
                b.align(*align).bold(*bold).size(scale, scale);
                b.line_in(text, cfg.cols() / usize::from(scale));
                b.normal();
            }
            Line::Row { left, right, bold } => {
                b.bold(*bold).row(left, right).bold(false);
            }
            Line::Rule => {
                b.rule();
            }
        }
    }
    finish(cfg, b)
}

fn encode_image(cfg: &PrintSettings, bitmap: &Bitmap) -> Vec<u8> {
    let mut b = Builder::new(cfg.cols(), cfg.codepage);
    b.raw_bytes(&bitmap.to_escpos());
    finish(cfg, b)
}

/// Printer bytes for a bill, as a picture or as text depending on the printer settings.
pub fn encode(cfg: &PrintSettings, font: Option<&Path>, doc: &Doc) -> Result<Vec<u8>> {
    match cfg.style {
        Style::Text => Ok(encode_text(cfg, doc)),
        Style::Image => Ok(encode_image(
            cfg,
            &raster::render(doc, cfg.dots(), cfg.shift, font)?,
        )),
    }
}

/// The picture that image printing would send (the settings preview shows exactly this).
pub fn preview(
    cfg: &PrintSettings,
    font: Option<&Path>,
    bill: &BillSettings,
    shop: &str,
    kind: Kind,
) -> Result<Bitmap> {
    let doc = document(
        bill,
        shop,
        &bill::sample_order(bill),
        Some("kbank"),
        kind,
        &now_stamp(),
    );
    raster::render(&doc, cfg.dots(), cfg.shift, font)
}

/// A page showing where the paper edges and the bill's centre fall (image style only).
pub fn calibration_job(cfg: &PrintSettings) -> Vec<u8> {
    encode_image(cfg, &raster::calibration(cfg.dots(), cfg.shift))
}

/// One Thai sample line per candidate code page, so the right number can be picked by eye.
pub fn codepage_test(cfg: &PrintSettings) -> Vec<u8> {
    let mut bytes = Vec::new();
    for page in CODEPAGE_CANDIDATES {
        let mut b = Builder::new(cfg.cols(), page);
        b.line(&format!("[{page}] ทดสอบ ที่ ปู่ ฿45.50"));
        bytes.extend(b.finish());
    }
    let mut end = Builder::new(cfg.cols(), cfg.codepage);
    end.line("เลือกเลขที่อ่านภาษาไทยถูกต้อง").feed(3);
    if cfg.cut {
        end.cut();
    }
    bytes.extend(end.finish());
    bytes
}

// --------------------------------------------------------------- transport

/// `COM5` -> `\\.\COM5` (also valid for COM10 and above). Anything else is refused so the
/// settings can never point the server at an arbitrary file or device.
pub fn com_path(target: &str) -> Option<String> {
    let upper = target.trim().to_ascii_uppercase();
    let number = upper.strip_prefix("COM")?;
    let valid = (1..=3).contains(&number.len()) && number.bytes().all(|b| b.is_ascii_digit());
    valid.then(|| format!(r"\\.\{upper}"))
}

fn send_blocking(cfg: &PrintSettings, dump: Option<&Path>, bytes: &[u8]) -> Result<()> {
    if let Some(path) = dump {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("เปิดไฟล์ {} ไม่ได้", path.display()))?;
        return file.write_all(bytes).context("เขียนไฟล์ทดลองพิมพ์ไม่ได้");
    }
    match cfg.mode {
        Mode::Off => bail!("ยังไม่ได้เปิดใช้เครื่องพิมพ์ ไปที่ ตั้งค่า > เครื่องพิมพ์"),
        Mode::Web => bail!("โหมดพิมพ์ผ่านเบราว์เซอร์: เซิร์ฟเวอร์สั่งพิมพ์เองไม่ได้"),
        #[cfg(not(windows))]
        Mode::Printer => bail!("เครื่องพิมพ์ที่ติดตั้งในระบบ ใช้ได้เฉพาะเซิร์ฟเวอร์ที่เป็น Windows"),
        #[cfg(windows)]
        Mode::Printer => {
            let printer = printers::get_printer_by_name(&cfg.target)
                .ok_or_else(|| anyhow!("ไม่พบเครื่องพิมพ์ “{}” ในเครื่องนี้", cfg.target))?;
            let options = PrinterJobOptions {
                name: Some(JOB_NAME),
                raw_properties: &[("document-format", "RAW")],
                converter: Converter::None,
            };
            printer
                .print(bytes, options)
                .map_err(|e| anyhow!("ส่งงานพิมพ์ไม่สำเร็จ: {e:?}"))?;
            Ok(())
        }
        Mode::Com => {
            let path = com_path(&cfg.target).ok_or_else(|| anyhow!("ชื่อพอร์ตต้องเป็น COM1, COM5 …"))?;
            let mut port = std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .with_context(|| {
                    format!(
                        "เปิดพอร์ต {} ไม่ได้ (ไม่ได้เชื่อมต่อ หรือถูกโปรแกรมอื่นใช้อยู่)",
                        cfg.target
                    )
                })?;
            port.write_all(bytes)
                .and_then(|()| port.flush())
                .with_context(|| format!("ส่งข้อมูลไปพอร์ต {} ไม่สำเร็จ", cfg.target))
        }
    }
}

/// Sends one sheet; sends are serialized so two jobs never interleave on the paper.
pub async fn send(state: &AppState, bytes: Vec<u8>) -> Result<()> {
    let _turn = state.print_lock().lock().await;
    let cfg = state.print();
    let dump = state.cfg.print_dump.clone();
    tokio::task::spawn_blocking(move || send_blocking(&cfg, dump.as_deref(), &bytes))
        .await
        .context("print task failed")?
}

#[cfg(not(windows))]
pub fn list_printers() -> Vec<String> {
    Vec::new()
}

#[cfg(windows)]
pub fn list_printers() -> Vec<String> {
    let mut names: Vec<String> = printers::get_printers()
        .into_iter()
        .map(|p| p.name)
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(windows)]
pub async fn list_com_ports() -> Vec<String> {
    use std::time::Duration;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SCRIPT: &str = "[System.IO.Ports.SerialPort]::GetPortNames() | Sort-Object";
    let mut command = tokio::process::Command::new("powershell");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(8), command.output()).await;
    match output {
        Ok(Ok(out)) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| com_path(l).is_some())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(not(windows))]
pub async fn list_com_ports() -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------- triggers

async fn payment_bank(pool: &SqlitePool, order_id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT bank FROM payments WHERE order_id = ? AND bank IS NOT NULL LIMIT 1")
        .bind(order_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// The printer bytes of one sheet of an order (the picture is made off the async threads).
pub async fn render_sheet(
    state: &AppState,
    order: &OrderView,
    kind: Kind,
    stamp: &str,
) -> Result<Vec<u8>> {
    let bank = payment_bank(&state.pool, order.id).await;
    let doc = document(
        &state.bill(),
        &state.shop.name,
        order,
        bank.as_deref(),
        kind,
        stamp,
    );
    encode_async(state, doc).await
}

async fn encode_async(state: &AppState, doc: Doc) -> Result<Vec<u8>> {
    let cfg = state.print();
    let font = state.cfg.print_font.clone();
    tokio::task::spawn_blocking(move || encode(&cfg, font.as_deref(), &doc))
        .await
        .context("print layout task failed")?
}

/// A sample bill with the saved wording as printer bytes.
pub async fn sample_sheet(state: &AppState, kind: Kind) -> Result<Vec<u8>> {
    let bill = state.bill();
    let doc = document(
        &bill,
        &state.shop.name,
        &bill::sample_order(&bill),
        Some("kbank"),
        kind,
        &now_stamp(),
    );
    encode_async(state, doc).await
}

/// Draws and sends one sheet of an order; the picture is made off the async threads.
pub async fn print_sheet(
    state: &AppState,
    order: &OrderView,
    kind: Kind,
    stamp: &str,
) -> Result<()> {
    let bank = payment_bank(&state.pool, order.id).await;
    let doc = document(
        &state.bill(),
        &state.shop.name,
        order,
        bank.as_deref(),
        kind,
        stamp,
    );
    send_doc(state, doc).await
}

pub async fn send_doc(state: &AppState, doc: Doc) -> Result<()> {
    let cfg = state.print();
    let font = state.cfg.print_font.clone();
    let sheet = tokio::task::spawn_blocking(move || encode(&cfg, font.as_deref(), &doc))
        .await
        .context("print layout task failed")??;
    send(state, sheet).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> PrintSettings {
        PrintSettings {
            mode: Mode::Printer,
            target: "POS-58".into(),
            ..PrintSettings::default()
        }
    }

    #[test]
    fn automatic_printing_follows_the_settings() {
        let names = |cfg: &PrintSettings| -> Vec<&'static str> {
            kinds_for(cfg).into_iter().map(Kind::as_str).collect()
        };
        let base = PrintSettings::default();
        assert_eq!(names(&base), ["ticket", "slip"]);
        let bill_only = PrintSettings {
            order_slip: false,
            ..base.clone()
        };
        assert_eq!(names(&bill_only), ["ticket"]);
        let none = PrintSettings {
            ticket: false,
            order_slip: false,
            ..base
        };
        assert!(names(&none).is_empty());
    }

    #[test]
    fn slip_prints_big_text_without_prices() {
        let bytes = encode(&text_cfg(), None, &sample(Kind::Slip)).unwrap();
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"A001"));
        assert!(has(&[0x1D, b'!', 0x11]), "2x size for the items");
        assert!(!has(b"110.00"));
    }

    #[test]
    fn com_path_accepts_only_com_ports() {
        assert_eq!(com_path("COM5").as_deref(), Some(r"\\.\COM5"));
        assert_eq!(com_path(" com12 ").as_deref(), Some(r"\\.\COM12"));
        assert_eq!(com_path("COM"), None);
        assert_eq!(com_path("COM1234"), None);
        assert_eq!(com_path(r"C:\Windows\win.ini"), None);
        assert_eq!(com_path(r"\\.\PhysicalDrive0"), None);
    }

    #[test]
    fn settings_round_trip_and_defaults() {
        let defaults = PrintSettings::from_map(&HashMap::new());
        assert_eq!(defaults.mode, Mode::Off);
        assert_eq!(defaults.style, Style::Image);
        assert_eq!(defaults.cols(), 32);
        assert!(!defaults.ready());
        let saved = PrintSettings {
            mode: Mode::Com,
            target: "COM7".into(),
            width: 80,
            codepage: 255,
            ticket: false,
            style: Style::Text,
            ..PrintSettings::default()
        };
        let map: HashMap<String, String> = saved
            .pairs()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let back = PrintSettings::from_map(&map);
        assert_eq!(back.mode, Mode::Com);
        assert_eq!((back.width, back.codepage, back.ticket), (80, 255, false));
        assert_eq!((back.style, back.dots()), (Style::Text, 576));
        assert!(back.order_slip);
        assert_eq!((back.head_dots, back.shift, back.gap_secs), (0, 0, 4));
        assert_eq!(back.cols(), 48);
        assert!(back.ready());
        let tuned = PrintSettings {
            head_dots: 400,
            shift: -16,
            gap_secs: 0,
            ..PrintSettings::default()
        };
        let map: HashMap<String, String> = tuned
            .pairs()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let back = PrintSettings::from_map(&map);
        assert_eq!((back.dots(), back.shift, back.gap_secs), (400, -16, 0));
    }

    fn text_cfg() -> PrintSettings {
        PrintSettings {
            style: Style::Text,
            ..cfg()
        }
    }

    fn sample(kind: Kind) -> Doc {
        let bill = BillSettings::default();
        document(
            &bill,
            "ร้านทดสอบ",
            &bill::sample_order(&bill),
            Some("kbank"),
            kind,
            "22/11/2569 15:29",
        )
    }

    #[test]
    fn text_ticket_contains_queue_items_total_and_cut() {
        let bytes = encode(&text_cfg(), None, &sample(Kind::Ticket)).unwrap();
        assert_eq!(&bytes[..5], &[0x1B, b'@', 0x1B, b't', 26]);
        assert!(bytes.ends_with(&[0x1D, b'V', 66, 3]));
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"A001"));
        assert!(has(b"125.00"));
        assert!(has(&crate::escpos::tis620("บัตรคิว")));
    }

    #[test]
    fn text_bill_shows_items_cash_and_change() {
        let bytes = encode(&text_cfg(), None, &sample(Kind::Ticket)).unwrap();
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"110.00"));
        assert!(has(b"200.00"), "cash received");
        assert!(has(b"75.00"), "change");
        assert!(has(&crate::escpos::tis620("เงินทอน")));
        // The old receipt name prints the very same bill.
        assert_eq!(
            encode(&text_cfg(), None, &sample(Kind::Receipt))
                .unwrap()
                .len(),
            bytes.len()
        );
    }

    #[test]
    fn no_cut_when_disabled() {
        let cfg = PrintSettings {
            cut: false,
            ..text_cfg()
        };
        let bytes = encode(&cfg, None, &sample(Kind::Receipt)).unwrap();
        assert!(!bytes.ends_with(&[0x1D, b'V', 66, 3]));
    }

    #[test]
    fn image_style_sends_raster_commands_instead_of_text() {
        let Ok(bytes) = encode(&cfg(), None, &sample(Kind::Ticket)) else {
            return; // no Thai font installed on this machine
        };
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(
            has(&[0x1D, b'v', b'0', 0, 48, 0]),
            "384 dots = 48 bytes per row"
        );
        assert!(!has(&crate::escpos::tis620("บัตรคิว")));
        assert!(bytes.ends_with(&[0x1D, b'V', 66, 3]));
    }

    #[test]
    fn codepage_test_prints_every_candidate() {
        let bytes = codepage_test(&cfg());
        for page in CODEPAGE_CANDIDATES {
            let needle = [0x1B, b't', page];
            assert!(bytes.windows(3).any(|w| w == needle), "page {page}");
        }
    }

    #[test]
    fn dump_file_receives_the_bytes() {
        let dir = std::env::temp_dir().join(format!("posmini-print-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("job.bin");
        let _ = std::fs::remove_file(&file);
        send_blocking(&cfg(), Some(&file), b"abc").unwrap();
        send_blocking(&cfg(), Some(&file), b"def").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"abcdef");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
