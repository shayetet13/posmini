fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn field(id: &str, value: &str) -> String {
    format!("{id}{:02}{value}", value.len())
}

fn normalize_target(raw: &str) -> Option<(&'static str, String)> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    match digits.len() {
        10 if digits.starts_with('0') => Some(("01", format!("0066{}", &digits[1..]))),
        13 => Some(("02", digits)),
        15 => Some(("03", digits)),
        _ => None,
    }
}

pub fn payload(promptpay_id: &str, amount_satang: i64) -> Option<String> {
    let (kind, target) = normalize_target(promptpay_id)?;
    if amount_satang <= 0 {
        return None;
    }
    let amount = format!("{}.{:02}", amount_satang / 100, amount_satang % 100);
    let merchant = field(
        "29",
        &format!(
            "{}{}",
            field("00", "A000000677010111"),
            field(kind, &target)
        ),
    );
    let body = format!(
        "{}{}{}{}{}{}6304",
        field("00", "01"),
        field("01", "12"),
        merchant,
        field("58", "TH"),
        field("53", "764"),
        field("54", &amount),
    );
    Some(format!("{body}{:04X}", crc16_ccitt(body.as_bytes())))
}

/// Highest error correction, so the Thai QR Payment mark can sit over the centre of the code.
pub fn qr_svg(payload: &str) -> Option<String> {
    let code =
        qrcode::QrCode::with_error_correction_level(payload.as_bytes(), qrcode::EcLevel::H).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color>()
            .min_dimensions(240, 240)
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_standard_check_value() {
        assert_eq!(crc16_ccitt(b"123456789"), 0x29B1);
    }

    #[test]
    fn builds_phone_payload_with_amount() {
        let p = payload("081-234-5678", 5_500).unwrap();
        assert!(p.starts_with("000201010212"));
        assert!(p.contains("0016A000000677010111"));
        assert!(p.contains("01130066812345678"));
        assert!(p.contains("540555.00"));
        let (body, crc) = p.split_at(p.len() - 4);
        assert_eq!(format!("{:04X}", crc16_ccitt(body.as_bytes())), crc);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(payload("123", 100).is_none());
        assert!(payload("0812345678", 0).is_none());
    }

    #[test]
    fn renders_svg() {
        let p = payload("0812345678", 100).unwrap();
        assert!(qr_svg(&p).unwrap().contains("<svg"));
    }
}
