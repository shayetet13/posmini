use axum::{
    Json,
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    admin::clean_text,
    auth::{Admin, Authed},
    bill::{self, BillSettings},
    error::{ApiError, ApiResult},
    escpos::is_zero_width,
    printing::{self, Kind, Mode, PrintSettings, Style},
    printq,
    state::AppState,
};

const TARGET_MAX_CHARS: usize = 100;
const LINE_MAX_CHARS: usize = 48;
const SHOP_NAME_MAX_CHARS: usize = 40;
const TITLE_MAX_CHARS: usize = 24;
const FOOTER_MAX_LINES: usize = 2;

#[derive(Deserialize)]
pub struct PrintSettingsReq {
    mode: String,
    target: String,
    width: u8,
    codepage: u8,
    ticket: bool,
    cut: bool,
    style: String,
    order_slip: bool,
    head_dots: usize,
    shift: i32,
    gap_secs: u64,
}

fn bad(message: &'static str) -> ApiError {
    ApiError::BadRequest(message)
}

fn settings_from_req(req: PrintSettingsReq) -> ApiResult<PrintSettings> {
    let mode = Mode::parse(&req.mode).ok_or(bad("วิธีเชื่อมต่อเครื่องพิมพ์ไม่ถูกต้อง"))?;
    if !matches!(req.width, 58 | 80) {
        return Err(bad("ขนาดกระดาษต้องเป็น 58 หรือ 80 มม."));
    }
    let target =
        clean_text(&req.target, TARGET_MAX_CHARS).ok_or(bad("ชื่อเครื่องพิมพ์หรือพอร์ตไม่ถูกต้อง"))?;
    let target = match mode {
        Mode::Off => target,
        Mode::Printer if target.is_empty() => return Err(bad("เลือกเครื่องพิมพ์ก่อน")),
        Mode::Printer => target,
        Mode::Web => String::new(),
        Mode::Com => printing::com_path(&target)
            .map(|_| target.to_ascii_uppercase())
            .ok_or(bad("ชื่อพอร์ตต้องเป็น COM1, COM5 …"))?,
    };

    let style = Style::parse(&req.style).ok_or(bad("รูปแบบการพิมพ์ไม่ถูกต้อง"))?;
    if req.head_dots != 0
        && !(printing::MIN_HEAD_DOTS..=printing::MAX_HEAD_DOTS).contains(&req.head_dots)
    {
        return Err(bad("ความกว้างที่พิมพ์ได้ต้องอยู่ระหว่าง 240-832 จุด (หรือ 0 = อัตโนมัติ)"));
    }
    if req.shift.abs() > printing::MAX_SHIFT_DOTS {
        return Err(bad("เลื่อนตำแหน่งได้ไม่เกิน 120 จุดในแต่ละด้าน"));
    }
    if req.gap_secs > printing::MAX_GAP_SECS {
        return Err(bad("เวลาหน่วงระหว่างใบต้องไม่เกิน 30 วินาที"));
    }

    Ok(PrintSettings {
        mode,
        target,
        width: req.width,
        codepage: req.codepage,
        ticket: req.ticket,
        cut: req.cut,
        style,
        order_slip: req.order_slip,
        head_dots: req.head_dots,
        shift: req.shift,
        gap_secs: req.gap_secs,
    })
}

fn view(cfg: &PrintSettings) -> Value {
    json!({
        "mode": cfg.mode.as_str(),
        "target": cfg.target,
        "width": cfg.width,
        "codepage": cfg.codepage,
        "ticket": cfg.ticket,
        "cut": cfg.cut,
        "style": cfg.style.as_str(),
        "order_slip": cfg.order_slip,
        "head_dots": cfg.head_dots,
        "dots": cfg.dots(),
        "shift": cfg.shift,
        "gap_secs": cfg.gap_secs,
        "ready": cfg.ready(),
        "codepages": printing::CODEPAGE_CANDIDATES,
    })
}

pub async fn get_settings(_auth: Authed, State(state): State<AppState>) -> Json<Value> {
    Json(view(&state.print()))
}

pub async fn put_settings(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<PrintSettingsReq>,
) -> ApiResult<Json<Value>> {
    let next = settings_from_req(req)?;
    printing::save(&state.pool, state.shop.id, &next).await?;
    let body = view(&next);
    state.set_print(next);
    Ok(Json(body))
}

/// Printers installed in Windows and serial (COM) ports, for the settings dropdowns.
pub async fn targets(_admin: Admin) -> Json<Value> {
    let ports = printing::list_com_ports().await;
    let printers = tokio::task::spawn_blocking(printing::list_printers)
        .await
        .unwrap_or_default();
    Json(json!({ "printers": printers, "ports": ports }))
}

#[derive(Deserialize)]
pub struct TestReq {
    kind: String,
}

fn require_ready(state: &AppState) -> ApiResult<()> {
    if state.print().ready() {
        Ok(())
    } else {
        Err(bad("ยังไม่ได้ตั้งค่าเครื่องพิมพ์ ไปที่ ตั้งค่า > เครื่องพิมพ์"))
    }
}

fn failed(err: &anyhow::Error) -> ApiError {
    tracing::error!(error = %format!("{err:#}"), "print failed");
    ApiError::PrintFailed(err.to_string())
}

pub async fn test(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<TestReq>,
) -> ApiResult<Json<Value>> {
    require_ready(&state)?;
    let cfg = state.print();
    let sheet = match req.kind.as_str() {
        "ticket" => printing::sample_sheet(&state, Kind::Ticket).await,
        "slip" => printing::sample_sheet(&state, Kind::Slip).await,
        "position" => Ok(printing::calibration_job(&cfg)),
        "codepages" => Ok(printing::codepage_test(&cfg)),
        _ => return Err(bad("ชนิดการทดสอบไม่ถูกต้อง")),
    }
    .map_err(|e| failed(&e))?;
    if cfg.mode == Mode::Web {
        // The browser on the shop's computer prints it; hand the bytes back to the caller.
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.encode(sheet);
        return Ok(Json(json!({ "ok": true, "web": true, "bytes": bytes })));
    }
    printing::send(&state, sheet)
        .await
        .map_err(|e| failed(&e))?;
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------------- queue

#[derive(Deserialize)]
pub struct ClaimReq {
    station: String,
}

const STATION_ID_MAX: usize = 64;

/// A browser print station asks for the next sheet (204 when there is nothing to print).
pub async fn station_claim(
    _auth: Authed,
    State(state): State<AppState>,
    Json(req): Json<ClaimReq>,
) -> ApiResult<Response> {
    if req.station.is_empty() || req.station.len() > STATION_ID_MAX {
        return Err(bad("รหัสสถานีพิมพ์ไม่ถูกต้อง"));
    }
    if state.print().mode != Mode::Web {
        return Err(bad("ยังไม่ได้เลือกโหมดพิมพ์ผ่านเบราว์เซอร์ในหน้าตั้งค่า"));
    }
    match printq::claim(&state, &req.station)
        .await
        .map_err(|e| failed(&e))?
    {
        Some(claim) => Ok(Json(claim).into_response()),
        None => Ok(axum::http::StatusCode::NO_CONTENT.into_response()),
    }
}

#[derive(Deserialize)]
pub struct DoneReq {
    job_id: i64,
}

pub async fn station_done(
    _auth: Authed,
    State(state): State<AppState>,
    Json(req): Json<DoneReq>,
) -> ApiResult<Json<Value>> {
    printq::done(&state, req.job_id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct FailReq {
    job_id: i64,
    message: String,
}

const FAIL_MESSAGE_MAX: usize = 200;

pub async fn station_fail(
    _auth: Authed,
    State(state): State<AppState>,
    Json(req): Json<FailReq>,
) -> ApiResult<Json<Value>> {
    let message: String = req.message.chars().take(FAIL_MESSAGE_MAX).collect();
    printq::fail(&state, req.job_id, &message).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn queue_status(_auth: Authed, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(printq::summary(&state).await?))
}

/// Gives up on the sheet at the head of the queue (e.g. a stuck job) so the next one can print.
pub async fn queue_skip(_auth: Authed, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    printq::skip_head(&state).await?;
    Ok(Json(printq::summary(&state).await?))
}

/// Puts sheets that were given up on back in line (in their original order).
pub async fn queue_retry(_auth: Authed, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    printq::retry_failed(&state).await?;
    Ok(Json(printq::summary(&state).await?))
}

// ------------------------------------------------------------------- bill

#[derive(Deserialize)]
pub struct BillReq {
    shop_name: String,
    header: String,
    footer: String,
    ticket_title: String,
    slip_title: String,
    ticket_note: String,
    queue_prefix: String,
    queue_digits: usize,
    show_time: bool,
    show_method: bool,
    show_order_no: bool,
    #[serde(default)]
    display_name: String,
}

/// Trimmed non-empty lines, each at most `LINE_MAX_CHARS`, at most `max_lines` of them.
fn lines_of(
    text: &str,
    max_lines: usize,
    too_many: &'static str,
    too_long: &'static str,
) -> ApiResult<String> {
    let lines: Vec<String> = text
        .lines()
        .map(|l| clean_text(l, LINE_MAX_CHARS))
        .collect::<Option<Vec<_>>>()
        .ok_or(bad(too_long))?
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() > max_lines {
        return Err(bad(too_many));
    }
    Ok(lines.join("\n"))
}

fn bill_from_req(req: BillReq) -> ApiResult<BillSettings> {
    let shop_name = clean_text(&req.shop_name, SHOP_NAME_MAX_CHARS)
        .ok_or(bad("ชื่อร้านบนบิลต้องไม่เกิน 40 ตัวอักษร"))?;
    let title =
        |text: &str| clean_text(text, TITLE_MAX_CHARS).ok_or(bad("หัวข้อบิลต้องไม่เกิน 24 ตัวอักษร"));
    let display_name = clean_text(&req.display_name, SHOP_NAME_MAX_CHARS)
        .ok_or(bad("ชื่อบนจอลูกค้าต้องไม่เกิน 40 ตัวอักษร"))?;
    let ticket_note = clean_text(&req.ticket_note, LINE_MAX_CHARS)
        .ok_or(bad("ข้อความใต้บัตรคิวต้องไม่เกิน 48 ตัวอักษร"))?;
    let prefix = req.queue_prefix.trim();
    let prefix_ok = prefix.chars().count() <= bill::MAX_QUEUE_PREFIX
        && prefix
            .chars()
            .all(|c| c.is_alphanumeric() || is_zero_width(c));
    if !prefix_ok {
        return Err(bad(
            "ตัวอักษรหน้าเลขคิวใช้ได้ไม่เกิน 3 ตัว (ตัวอักษรหรือตัวเลข ไม่มีเว้นวรรค)",
        ));
    }
    if !(1..=bill::MAX_QUEUE_DIGITS).contains(&req.queue_digits) {
        return Err(bad("จำนวนหลักของเลขคิวต้องอยู่ระหว่าง 1-5"));
    }
    Ok(BillSettings {
        shop_name,
        header: lines_of(
            &req.header,
            bill::MAX_HEADER_LINES,
            "หัวบิลใส่ได้ไม่เกิน 4 บรรทัด",
            "หัวบิลแต่ละบรรทัดต้องไม่เกิน 48 ตัวอักษร",
        )?,
        footer: lines_of(
            &req.footer,
            FOOTER_MAX_LINES,
            "ท้ายบิลใส่ได้ไม่เกิน 2 บรรทัด",
            "ท้ายบิลแต่ละบรรทัดต้องไม่เกิน 48 ตัวอักษร",
        )?,
        ticket_title: title(&req.ticket_title)?,
        slip_title: title(&req.slip_title)?,
        ticket_note,
        queue_prefix: prefix.to_string(),
        queue_digits: req.queue_digits,
        show_time: req.show_time,
        show_method: req.show_method,
        show_order_no: req.show_order_no,
        display_name,
    })
}

fn bill_view(bill: &BillSettings, shop: &str) -> Value {
    json!({
        "shop_name": bill.shop_name,
        "shop_default": shop,
        "header": bill.header,
        "footer": bill.footer,
        "ticket_title": bill.ticket_title,
        "slip_title": bill.slip_title,
        "ticket_note": bill.ticket_note,
        "queue_prefix": bill.queue_prefix,
        "queue_digits": bill.queue_digits,
        "show_time": bill.show_time,
        "show_method": bill.show_method,
        "show_order_no": bill.show_order_no,
        "display_name": bill.display_name,
        "queue_example": bill.queue_label(7),
    })
}

pub async fn get_bill(_admin: Admin, State(state): State<AppState>) -> Json<Value> {
    Json(bill_view(&state.bill(), &state.shop.name))
}

pub async fn put_bill(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<BillReq>,
) -> ApiResult<Json<Value>> {
    let next = bill_from_req(req)?;
    bill::save(&state.pool, state.shop.id, &next).await?;
    let body = bill_view(&next, &state.shop.name);
    state.set_bill(next);
    // Queue labels on every open screen use the new format.
    state.publish(crate::events::Event::new("orders", json!({})));
    Ok(Json(body))
}

#[derive(Deserialize)]
pub struct PreviewReq {
    kind: String,
    #[serde(flatten)]
    bill: BillReq,
}

/// The bill exactly as it would print (a picture), for the wording currently typed in.
pub async fn preview(
    _admin: Admin,
    State(state): State<AppState>,
    Json(req): Json<PreviewReq>,
) -> ApiResult<Response> {
    let kind = match req.kind.as_str() {
        "ticket" => Kind::Ticket,
        "receipt" => Kind::Receipt,
        "slip" => Kind::Slip,
        _ => return Err(bad("ชนิดบิลไม่ถูกต้อง")),
    };
    let bill = bill_from_req(req.bill)?;
    let cfg = state.print();
    let font = state.cfg.print_font.clone();
    let shop = state.shop.name.clone();
    let bitmap = tokio::task::spawn_blocking(move || {
        printing::preview(&cfg, font.as_deref(), &bill, &shop, kind)
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))?
    .map_err(|e| failed(&e))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/bmp"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bitmap.to_bmp(),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct OrderPrintReq {
    kind: String,
}

/// Reprint the customer's bill or the shop's order slip from the cashier screen.
pub async fn print_order(
    _auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<OrderPrintReq>,
) -> ApiResult<Json<Value>> {
    require_ready(&state)?;
    let kind = match req.kind.as_str() {
        "ticket" => Kind::Ticket,
        "receipt" => Kind::Receipt,
        "slip" => Kind::Slip,
        _ => return Err(bad("ชนิดการพิมพ์ไม่ถูกต้อง")),
    };
    // A reprint goes to the back of the print queue like any other sheet.
    printq::enqueue_kinds(&state, id, &[kind])
        .await
        .map_err(|e| failed(&e))?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(mode: &str, target: &str) -> PrintSettingsReq {
        PrintSettingsReq {
            mode: mode.into(),
            target: target.into(),
            width: 58,
            codepage: 26,
            ticket: true,
            cut: true,
            style: "image".into(),
            order_slip: true,
            head_dots: 0,
            shift: 0,
            gap_secs: 4,
        }
    }

    fn bill_req() -> BillReq {
        BillReq {
            shop_name: " ร้านใหม่ ".into(),
            header: "ที่อยู่ร้าน\n\nโทร 081-234-5678\n".into(),
            footer: "ขอบคุณครับ".into(),
            ticket_title: "บัตรคิว".into(),
            slip_title: "ใบออเดอร์".into(),
            ticket_note: "รอเรียกคิว".into(),
            queue_prefix: "B".into(),
            queue_digits: 3,
            show_time: true,
            show_method: true,
            show_order_no: false,
            display_name: " จอลูกค้า ".into(),
        }
    }

    #[test]
    fn accepts_valid_and_normalizes() {
        let s = settings_from_req(req("com", "com5")).ok().unwrap();
        assert_eq!(s.target, "COM5");
        assert_eq!(s.style, Style::Image);
        assert!(s.ready());
        assert!(settings_from_req(req("printer", "POS-58")).is_ok());
        assert!(!settings_from_req(req("off", "")).ok().unwrap().ready());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(settings_from_req(req("usb", "x")).is_err());
        assert!(settings_from_req(req("printer", "")).is_err());
        assert!(settings_from_req(req("com", r"\\.\PhysicalDrive0")).is_err());
        assert!(settings_from_req(req("com", "C:\\x.txt")).is_err());
        let mut wide = req("printer", "P");
        wide.width = 72;
        assert!(settings_from_req(wide).is_err());
        let mut style = req("printer", "P");
        style.style = "pdf".into();
        assert!(settings_from_req(style).is_err());
        for (dots, shift, gap) in [
            (100, 0, 4),
            (900, 0, 4),
            (0, 121, 4),
            (0, -121, 4),
            (0, 0, 31),
        ] {
            let mut bad = req("printer", "P");
            (bad.head_dots, bad.shift, bad.gap_secs) = (dots, shift, gap);
            assert!(settings_from_req(bad).is_err(), "{dots} {shift} {gap}");
        }
        let mut tuned = req("printer", "P");
        (tuned.head_dots, tuned.shift, tuned.gap_secs) = (400, -20, 0);
        assert_eq!(settings_from_req(tuned).ok().unwrap().dots(), 400);
    }

    #[test]
    fn bill_is_cleaned_and_validated() {
        let ok = bill_from_req(bill_req()).ok().unwrap();
        assert_eq!(ok.shop_name, "ร้านใหม่");
        assert_eq!(ok.display_name, "จอลูกค้า");
        assert_eq!(ok.display_title("ชื่อร้าน"), "จอลูกค้า");
        assert_eq!(BillSettings::default().display_title("ชื่อร้าน"), "ชื่อร้าน");
        assert_eq!(ok.header, "ที่อยู่ร้าน\nโทร 081-234-5678");
        assert_eq!(ok.queue_label(5), "B005");

        let mut many = bill_req();
        many.header = "a\nb\nc\nd\ne".into();
        assert!(bill_from_req(many).is_err());
        let mut long = bill_req();
        long.footer = "ก".repeat(49);
        assert!(bill_from_req(long).is_err());
        let mut footer = bill_req();
        footer.footer = "a\nb\nc".into();
        assert!(bill_from_req(footer).is_err());
        let mut prefix = bill_req();
        prefix.queue_prefix = "AB-1".into();
        assert!(bill_from_req(prefix).is_err());
        let mut spaced = bill_req();
        spaced.queue_prefix = "A B".into();
        assert!(bill_from_req(spaced).is_err());
        let mut thai = bill_req();
        thai.queue_prefix = "คิว".into();
        assert!(bill_from_req(thai).is_ok());
        let mut digits = bill_req();
        digits.queue_digits = 0;
        assert!(bill_from_req(digits).is_err());
        let mut digits = bill_req();
        digits.queue_digits = 6;
        assert!(bill_from_req(digits).is_err());
        let mut name = bill_req();
        name.shop_name = "x".repeat(41);
        assert!(bill_from_req(name).is_err());
        let mut empty = bill_req();
        empty.queue_prefix = String::new();
        empty.ticket_title = String::new();
        assert!(bill_from_req(empty).is_ok());
    }
}
