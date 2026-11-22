//! What goes on a queue ticket / receipt: the editable wording and queue-number format, and
//! the printer-independent layout (`Doc`) that both the image and the text printer render.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::{
    escpos::Align,
    orders::{ItemView, OrderView},
    payment::format_baht,
    settings,
};

pub const DEFAULT_FOOTER: &str = "ขอบคุณที่อุดหนุน";
pub const DEFAULT_TICKET_TITLE: &str = "บัตรคิว / ใบเสร็จรับเงิน";
pub const DEFAULT_SLIP_TITLE: &str = "ใบออเดอร์ (สำหรับร้าน)";
pub const DEFAULT_TICKET_NOTE: &str = "กรุณารอเรียกคิว";
pub const DEFAULT_QUEUE_PREFIX: &str = "A";
pub const DEFAULT_QUEUE_DIGITS: usize = 3;
pub const MAX_QUEUE_PREFIX: usize = 3;
pub const MAX_QUEUE_DIGITS: usize = 5;
pub const MAX_HEADER_LINES: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BillSettings {
    /// Name printed at the top; empty means the shop name from `.env`.
    pub shop_name: String,
    /// Address / phone / tax id, one per line.
    pub header: String,
    pub footer: String,
    pub ticket_title: String,
    pub slip_title: String,
    pub ticket_note: String,
    pub queue_prefix: String,
    pub queue_digits: usize,
    pub show_time: bool,
    pub show_method: bool,
    pub show_order_no: bool,
}

impl Default for BillSettings {
    fn default() -> Self {
        Self {
            shop_name: String::new(),
            header: String::new(),
            footer: DEFAULT_FOOTER.to_string(),
            ticket_title: DEFAULT_TICKET_TITLE.to_string(),
            slip_title: DEFAULT_SLIP_TITLE.to_string(),
            ticket_note: DEFAULT_TICKET_NOTE.to_string(),
            queue_prefix: DEFAULT_QUEUE_PREFIX.to_string(),
            queue_digits: DEFAULT_QUEUE_DIGITS,
            show_time: true,
            show_method: true,
            show_order_no: true,
        }
    }
}

const KEYS: [&str; 11] = [
    "bill_shop_name",
    "bill_header",
    "bill_footer",
    "bill_ticket_title",
    "bill_ticket_note",
    "bill_queue_prefix",
    "bill_queue_digits",
    "bill_show_time",
    "bill_show_method",
    "bill_show_order_no",
    "bill_slip_title",
];

impl BillSettings {
    /// Header and footer used to live with the printer settings; older databases still have them.
    pub fn from_map(map: &HashMap<String, String>) -> Self {
        let base = Self::default();
        let text = |key: &str, legacy: Option<&str>, default: &str| {
            map.get(key)
                .or_else(|| legacy.and_then(|k| map.get(k)))
                .cloned()
                .unwrap_or_else(|| default.to_string())
        };
        let flag = |key: &str, default: bool| map.get(key).map_or(default, |v| v == "1");
        Self {
            shop_name: text(KEYS[0], None, &base.shop_name),
            header: text(KEYS[1], Some("print_header"), &base.header),
            footer: text(KEYS[2], Some("print_footer"), &base.footer),
            ticket_title: text(KEYS[3], None, &base.ticket_title),
            ticket_note: text(KEYS[4], None, &base.ticket_note),
            queue_prefix: text(KEYS[5], None, &base.queue_prefix),
            queue_digits: map
                .get(KEYS[6])
                .and_then(|v| v.parse().ok())
                .filter(|d| (1..=MAX_QUEUE_DIGITS).contains(d))
                .unwrap_or(base.queue_digits),
            show_time: flag(KEYS[7], base.show_time),
            show_method: flag(KEYS[8], base.show_method),
            show_order_no: flag(KEYS[9], base.show_order_no),
            slip_title: text(KEYS[10], None, &base.slip_title),
        }
    }

    fn pairs(&self) -> Vec<(&'static str, String)> {
        let flag = |on: bool| if on { "1" } else { "0" }.to_string();
        vec![
            (KEYS[0], self.shop_name.clone()),
            (KEYS[1], self.header.clone()),
            (KEYS[2], self.footer.clone()),
            (KEYS[3], self.ticket_title.clone()),
            (KEYS[4], self.ticket_note.clone()),
            (KEYS[5], self.queue_prefix.clone()),
            (KEYS[6], self.queue_digits.to_string()),
            (KEYS[7], flag(self.show_time)),
            (KEYS[8], flag(self.show_method)),
            (KEYS[9], flag(self.show_order_no)),
            (KEYS[10], self.slip_title.clone()),
        ]
    }

    /// `A001` with the default format; the prefix and digit count are editable.
    pub fn queue_label(&self, queue_no: i64) -> String {
        format!(
            "{}{:0>width$}",
            self.queue_prefix,
            queue_no,
            width = self.queue_digits
        )
    }

    pub fn shop_title<'a>(&'a self, fallback: &'a str) -> &'a str {
        let name = self.shop_name.trim();
        if name.is_empty() { fallback } else { name }
    }

    pub fn header_lines(&self) -> Vec<&str> {
        self.header
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect()
    }
}

pub async fn load(pool: &SqlitePool, shop_id: i64) -> Result<BillSettings> {
    Ok(BillSettings::from_map(
        &settings::load_map(pool, shop_id).await?,
    ))
}

pub async fn save(pool: &SqlitePool, shop_id: i64, bill: &BillSettings) -> Result<()> {
    settings::save_pairs(pool, shop_id, &bill.pairs()).await
}

// ------------------------------------------------------------------ layout

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Normal,
    /// Shop name.
    Title,
    /// Items on the shop's order slip: big, bold, wrapped rather than shrunk.
    Large,
    /// Queue number on the ticket.
    Huge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Text {
        text: String,
        size: Size,
        bold: bool,
        align: Align,
    },
    /// Left text and right-aligned text on one row (the left side wraps).
    Row {
        left: String,
        right: String,
        bold: bool,
    },
    Rule,
}

pub type Doc = Vec<Line>;

/// Per-print facts that are not part of the order or the settings.
pub struct Facts<'a> {
    pub shop: &'a str,
    pub bank: Option<&'a str>,
    /// Already formatted date/time, e.g. `22/11/2569 15:29`.
    pub stamp: &'a str,
}

fn text(text: &str, size: Size, bold: bool, align: Align) -> Line {
    Line::Text {
        text: text.to_string(),
        size,
        bold,
        align,
    }
}

fn plain(value: &str) -> Line {
    text(value, Size::Normal, false, Align::Left)
}

fn centered(value: &str, bold: bool) -> Line {
    text(value, Size::Normal, bold, Align::Center)
}

fn row(left: &str, right: &str, bold: bool) -> Line {
    Line::Row {
        left: left.to_string(),
        right: right.to_string(),
        bold,
    }
}

fn method_label(method: &str, bank: Option<&str>) -> String {
    match (method, bank) {
        ("cash", _) => "ชำระด้วย เงินสด".to_string(),
        (_, Some(bank)) => format!("ชำระด้วย พร้อมเพย์ ({})", bank.to_uppercase()),
        _ => "ชำระด้วย พร้อมเพย์".to_string(),
    }
}

fn masthead(bill: &BillSettings, facts: &Facts) -> Doc {
    let mut doc = vec![text(
        bill.shop_title(facts.shop),
        Size::Title,
        true,
        Align::Center,
    )];
    doc.extend(bill.header_lines().into_iter().map(|l| centered(l, false)));
    doc
}

fn closing(bill: &BillSettings, doc: &mut Doc, note: Option<&str>) {
    for line in note
        .into_iter()
        .chain(bill.footer.lines())
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        doc.push(centered(line, false));
    }
}

/// "ออเดอร์ #26", big and bold, right under the queue number.
fn order_no(order: &OrderView) -> Line {
    text(
        &format!("ออเดอร์ #{}", order.id),
        Size::Large,
        true,
        Align::Center,
    )
}

fn queue_number(order: &OrderView) -> Line {
    text(
        order.queue_label.as_deref().unwrap_or("-"),
        Size::Huge,
        true,
        Align::Center,
    )
}

/// The customer's bill: queue ticket and receipt in one. The queue and order numbers are the
/// biggest things on it; for cash it also shows what was handed over and the change.
pub fn ticket(bill: &BillSettings, facts: &Facts, order: &OrderView) -> Doc {
    let mut doc = masthead(bill, facts);
    doc.push(Line::Rule);
    if !bill.ticket_title.trim().is_empty() {
        doc.push(centered(bill.ticket_title.trim(), true));
    }
    doc.push(queue_number(order));
    if bill.show_order_no {
        doc.push(order_no(order));
    }
    doc.push(Line::Rule);
    for item in &order.items {
        doc.push(row(
            &format!("{} x{}", item.name, item.qty),
            &format_baht(item.unit_price_satang * item.qty),
            false,
        ));
    }
    doc.push(Line::Rule);
    doc.push(row("รวม", &format_baht(order.total_satang), true));
    match (order.method.as_str(), order.cash_received_satang) {
        ("cash", Some(received)) => {
            doc.push(row("รับเงินสด", &format_baht(received), false));
            doc.push(row(
                "เงินทอน",
                &format_baht(order.change_satang.unwrap_or(0)),
                true,
            ));
        }
        _ if bill.show_method => doc.push(plain(&method_label(&order.method, facts.bank))),
        _ => {}
    }
    if bill.show_time {
        doc.push(plain(facts.stamp));
    }
    closing(bill, &mut doc, Some(bill.ticket_note.as_str()));
    doc
}

/// Slip for the people making the order: big queue and order numbers, big items, no prices.
pub fn order_slip(bill: &BillSettings, facts: &Facts, order: &OrderView) -> Doc {
    let mut doc = Vec::new();
    if !bill.slip_title.trim().is_empty() {
        doc.push(centered(bill.slip_title.trim(), true));
    }
    doc.push(queue_number(order));
    doc.push(order_no(order));
    doc.push(Line::Rule);
    for item in &order.items {
        doc.push(text(
            &format!("{} x {}", item.qty, item.name),
            Size::Large,
            true,
            Align::Left,
        ));
    }
    doc.push(Line::Rule);
    let source = if order.source == "self" {
        "ลูกค้าสั่งเองผ่านมือถือ"
    } else {
        "สั่งหน้าร้าน"
    };
    doc.push(plain(&format!("{source} · ชำระแล้ว")));
    if bill.show_time {
        doc.push(plain(facts.stamp));
    }
    doc
}

/// A made-up order for the test print and the live preview.
pub fn sample_order(bill: &BillSettings) -> OrderView {
    let item = |name: &str, qty: i64, price: i64| ItemView {
        name: name.to_string(),
        qty,
        unit_price_satang: price,
    };
    OrderView {
        id: 17,
        token: String::new(),
        queue_no: Some(1),
        queue_label: Some(bill.queue_label(1)),
        status: "paid".to_string(),
        method: "cash".to_string(),
        cash_received_satang: Some(20_000),
        change_satang: Some(7_500),
        source: "pos".to_string(),
        total_satang: 12_500,
        pay_state: None,
        pay_note: None,
        qr_svg: None,
        qr_expires_in: None,
        ahead: None,
        created_at: String::new(),
        items: vec![
            item("ลาเต้เย็น ไม่หวาน", 2, 5_500),
            item("ปาท่องโก๋สังขยา", 1, 1_500),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts<'static> {
        Facts {
            shop: "ร้านทดสอบ",
            bank: Some("kbank"),
            stamp: "22/11/2569 15:29",
        }
    }

    fn texts(doc: &Doc) -> Vec<String> {
        doc.iter()
            .map(|l| match l {
                Line::Text { text, .. } => text.clone(),
                Line::Row { left, right, .. } => format!("{left}|{right}"),
                Line::Rule => "-".to_string(),
            })
            .collect()
    }

    #[test]
    fn queue_label_follows_prefix_and_digits() {
        let mut bill = BillSettings::default();
        assert_eq!(bill.queue_label(7), "A007");
        bill.queue_prefix = "คิว".into();
        bill.queue_digits = 2;
        assert_eq!(bill.queue_label(7), "คิว07");
        assert_eq!(bill.queue_label(123), "คิว123");
        bill.queue_prefix = String::new();
        bill.queue_digits = 1;
        assert_eq!(bill.queue_label(7), "7");
    }

    #[test]
    fn legacy_print_header_and_footer_are_still_used() {
        let map: HashMap<String, String> = [("print_header", "โทร 081"), ("print_footer", "ขอบคุณ")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let bill = BillSettings::from_map(&map);
        assert_eq!(bill.header, "โทร 081");
        assert_eq!(bill.footer, "ขอบคุณ");
        let mut newer = map;
        newer.insert("bill_footer".into(), String::new());
        assert_eq!(BillSettings::from_map(&newer).footer, "");
    }

    #[test]
    fn settings_round_trip() {
        let saved = BillSettings {
            shop_name: "ร้านใหม่".into(),
            header: "ที่อยู่\nโทร".into(),
            queue_prefix: "B".into(),
            queue_digits: 4,
            show_order_no: false,
            ..BillSettings::default()
        };
        let map: HashMap<String, String> = saved
            .pairs()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        assert_eq!(BillSettings::from_map(&map), saved);
        assert_eq!(
            BillSettings::from_map(&HashMap::new()),
            BillSettings::default()
        );
    }

    #[test]
    fn the_bill_is_one_sheet_with_big_numbers_and_the_change() {
        let bill = BillSettings::default();
        let order = sample_order(&bill);
        let doc = ticket(&bill, &facts(), &order);
        let lines = texts(&doc);
        assert!(lines.contains(&"A001".to_string()));
        assert!(lines.contains(&"ออเดอร์ #17".to_string()));
        assert!(lines.iter().any(|l| l.starts_with("ลาเต้เย็น ไม่หวาน x2|")));
        assert!(lines.contains(&"รวม|125.00".to_string()));
        assert!(lines.contains(&"รับเงินสด|200.00".to_string()));
        assert!(lines.contains(&"เงินทอน|75.00".to_string()));
        assert!(lines.contains(&"22/11/2569 15:29".to_string()));
        let sizes: Vec<Size> = doc
            .iter()
            .filter_map(|l| match l {
                Line::Text { size, .. } => Some(*size),
                _ => None,
            })
            .collect();
        assert!(sizes.contains(&Size::Huge) && sizes.contains(&Size::Large));
    }

    #[test]
    fn cash_without_a_received_amount_and_promptpay_show_the_payment_method() {
        let bill = BillSettings::default();
        let mut order = sample_order(&bill);
        order.cash_received_satang = None;
        order.change_satang = None;
        let lines = texts(&ticket(&bill, &facts(), &order));
        assert!(lines.contains(&"ชำระด้วย เงินสด".to_string()));
        assert!(!lines.iter().any(|l| l.starts_with("เงินทอน")));
        order.method = "promptpay".into();
        let lines = texts(&ticket(&bill, &facts(), &order));
        assert!(lines.contains(&"ชำระด้วย พร้อมเพย์ (KBANK)".to_string()));
    }

    #[test]
    fn toggles_hide_optional_lines_and_the_shop_name_can_be_replaced() {
        let bill = BillSettings {
            shop_name: "ร้านของฉัน".into(),
            header: "สาขา 1\n\nโทร 02".into(),
            show_time: false,
            show_order_no: false,
            ticket_title: String::new(),
            ..BillSettings::default()
        };
        let order = sample_order(&bill);
        let lines = texts(&ticket(&bill, &facts(), &order));
        assert_eq!(lines[0], "ร้านของฉัน");
        assert_eq!(&lines[1..3], ["สาขา 1", "โทร 02"]);
        assert!(!lines.contains(&"22/11/2569 15:29".to_string()));
        assert!(!lines.iter().any(|l| l.starts_with("ออเดอร์ #")));
        assert!(!lines.contains(&"บัตรคิว / ใบเสร็จรับเงิน".to_string()));
        let fallback = texts(&ticket(&BillSettings::default(), &facts(), &order));
        assert_eq!(fallback[0], "ร้านทดสอบ");
    }

    #[test]
    fn order_slip_has_big_items_and_no_prices() {
        let bill = BillSettings::default();
        let order = sample_order(&bill);
        let doc = order_slip(&bill, &facts(), &order);
        let lines = texts(&doc);
        assert!(lines.contains(&"A001".to_string()));
        assert!(lines.contains(&"ออเดอร์ #17".to_string()));
        assert!(lines.contains(&"2 x ลาเต้เย็น ไม่หวาน".to_string()));
        assert!(!lines.iter().any(|l| l.contains("110") || l.contains("125")));
        assert!(!lines.iter().any(|l| l.contains('|')), "no price rows");
        let mut by_phone = order;
        by_phone.source = "self".into();
        assert!(
            texts(&order_slip(&bill, &facts(), &by_phone))
                .iter()
                .any(|l| l.contains("มือถือ"))
        );
    }
}
