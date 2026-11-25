const DIGITS: [&str; 10] = [
    "ศูนย์",
    "หนึ่ง",
    "สอง",
    "สาม",
    "สี่",
    "ห้า",
    "หก",
    "เจ็ด",
    "แปด",
    "เก้า",
];
const PLACES: [&str; 6] = ["", "สิบ", "ร้อย", "พัน", "หมื่น", "แสน"];

fn below_million(n: u64, has_higher: bool) -> String {
    let digits: Vec<u64> = n.to_string().bytes().map(|b| u64::from(b - b'0')).collect();
    let len = digits.len();
    let mut out = String::new();
    for (i, &d) in digits.iter().enumerate() {
        let place = len - 1 - i;
        if d == 0 {
            continue;
        }
        match (place, d) {
            (1, 1) => {}
            (1, 2) => out.push_str("ยี่"),
            (0, 1) if len > 1 || has_higher => out.push_str("เอ็ด"),
            _ => out.push_str(DIGITS[d as usize]),
        }
        out.push_str(PLACES[place]);
    }
    out
}

pub fn number_words(n: u64) -> String {
    if n == 0 {
        return DIGITS[0].to_string();
    }
    let mut parts = Vec::new();
    let mut rest = n;
    while rest > 0 {
        parts.push(rest % 1_000_000);
        rest /= 1_000_000;
    }
    let mut out = String::new();
    for (i, chunk) in parts.iter().enumerate().rev() {
        if *chunk == 0 {
            continue;
        }
        out.push_str(&below_million(*chunk, !out.is_empty()));
        out.push_str(&"ล้าน".repeat(i));
    }
    out
}

pub fn baht_words(satang: i64) -> String {
    let satang = satang.max(0) as u64;
    let baht = satang / 100;
    let cents = satang % 100;
    match (baht, cents) {
        (0, 0) => "ศูนย์บาท".to_string(),
        (b, 0) => format!("{}บาท", number_words(b)),
        (0, c) => format!("{}สตางค์", number_words(c)),
        (b, c) => format!("{}บาท{}สตางค์", number_words(b), number_words(c)),
    }
}

// Every line starts with a throwaway lead-in ("ได้รับเงิน", "คุณลูกค้าคะ"): the first moment of
// the voice is often faint or lost while the speaker wakes up, and this way it is the lead-in
// that is lost. The browser voice pauses at every space, so a space is a deliberate pause and
// the rest of each line runs together.
pub fn payment_phrase(satang: i64) -> String {
    format!("ได้รับเงิน{}ลุยต่อเลย", baht_words(satang))
}

pub fn queue_phrase(queue_no: i64) -> String {
    format!(
        "คุณลูกค้าคะ คิวที่{}ได้เวลา กินชาแล้ว",
        number_words(queue_no.max(0) as u64)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_small_numbers() {
        assert_eq!(number_words(0), "ศูนย์");
        assert_eq!(number_words(5), "ห้า");
        assert_eq!(number_words(10), "สิบ");
        assert_eq!(number_words(11), "สิบเอ็ด");
        assert_eq!(number_words(20), "ยี่สิบ");
        assert_eq!(number_words(21), "ยี่สิบเอ็ด");
        assert_eq!(number_words(55), "ห้าสิบห้า");
    }

    #[test]
    fn reads_larger_numbers() {
        assert_eq!(number_words(101), "หนึ่งร้อยเอ็ด");
        assert_eq!(number_words(1_000), "หนึ่งพัน");
        assert_eq!(number_words(12_345), "หนึ่งหมื่นสองพันสามร้อยสี่สิบห้า");
        assert_eq!(number_words(1_000_000), "หนึ่งล้าน");
        assert_eq!(number_words(1_000_001), "หนึ่งล้านเอ็ด");
    }

    #[test]
    fn reads_baht_and_satang() {
        assert_eq!(baht_words(5_500), "ห้าสิบห้าบาท");
        assert_eq!(baht_words(5_525), "ห้าสิบห้าบาทยี่สิบห้าสตางค์");
        assert_eq!(baht_words(50), "ห้าสิบสตางค์");
        assert_eq!(baht_words(0), "ศูนย์บาท");
    }

    #[test]
    fn builds_phrases() {
        assert_eq!(payment_phrase(11_000), "ได้รับเงินหนึ่งร้อยสิบบาทลุยต่อเลย");
        assert_eq!(payment_phrase(5_525), "ได้รับเงินห้าสิบห้าบาทยี่สิบห้าสตางค์ลุยต่อเลย");
        assert_eq!(queue_phrase(12), "คุณลูกค้าคะ คิวที่สิบสองได้เวลา กินชาแล้ว");
        // The spaces are the pauses: none in the payment line, two in the call line.
        assert_eq!(payment_phrase(525).matches(' ').count(), 0);
        assert_eq!(queue_phrase(105).matches(' ').count(), 2);
    }
}
