//! The menu tree and the card colours. A group (`is_group`) holds items and other groups through
//! `parent_id`; rows with no parent are the top level. Every row has its own colour and, among
//! the rows shown side by side (same parent), no two share one, so each card is easy to recognise.

use std::collections::HashSet;

use anyhow::Result;
use sqlx::SqlitePool;

/// Easy-to-tell-apart card colours. The page picks black or white text from the brightness.
pub const PALETTE: [&str; 24] = [
    "#E5484D", "#F76B15", "#FFB224", "#F5D90A", "#99D52A", "#46A758", "#12A594", "#05A2C2",
    "#0091FF", "#3E63DD", "#6E56CF", "#8E4EC6", "#D6409F", "#E93D82", "#AD7F58", "#7D8590",
    "#C2410C", "#0E7490", "#15803D", "#A16207", "#BE185D", "#4338CA", "#9F1239", "#1F2937",
];

/// `#RRGGBB` and nothing else.
pub fn is_hex_color(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Colours are stored upper-case so two spellings of one colour count as the same.
pub fn normal_color(text: &str) -> Option<String> {
    let text = text.trim();
    is_hex_color(text).then(|| text.to_ascii_uppercase())
}

fn hsl_to_hex(hue: f64, sat: f64, light: f64) -> String {
    let chroma = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let x = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = light - chroma / 2.0;
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let byte = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02X}{:02X}{:02X}", byte(r), byte(g), byte(b))
}

/// The first palette colour nobody next to it uses; when the palette is used up, hues are spread
/// evenly around the colour wheel so there is always a new one.
pub fn pick_color(used: &HashSet<String>) -> String {
    if let Some(free) = PALETTE.iter().find(|c| !used.contains(**c)) {
        return (*free).to_string();
    }
    (0_u32..)
        .map(|n| hsl_to_hex(f64::from(n) * 137.508 % 360.0, 0.62, 0.42))
        .find(|c| !used.contains(c))
        .unwrap_or_else(|| PALETTE[0].to_string())
}

/// (id, name, colour) of the active rows under `parent` (`None` = top level).
pub async fn siblings(
    pool: &SqlitePool,
    shop_id: i64,
    parent: Option<i64>,
) -> Result<Vec<(i64, String, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT id, name, color FROM products \
         WHERE shop_id = ? AND active = 1 AND parent_id IS ? ORDER BY sort, id",
    )
    .bind(shop_id)
    .bind(parent)
    .fetch_all(pool)
    .await?)
}

/// Gives every row that has no colour yet one that its neighbours do not use (older menus).
pub async fn ensure_colors(pool: &SqlitePool, shop_id: i64) -> Result<()> {
    let parents: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT DISTINCT parent_id FROM products WHERE shop_id = ? AND active = 1 AND color IS NULL",
    )
    .bind(shop_id)
    .fetch_all(pool)
    .await?;
    for parent in parents {
        let rows = siblings(pool, shop_id, parent).await?;
        let mut used: HashSet<String> = rows.iter().filter_map(|(_, _, c)| c.clone()).collect();
        for (id, _, color) in rows {
            if color.is_some() {
                continue;
            }
            let pick = pick_color(&used);
            used.insert(pick.clone());
            sqlx::query("UPDATE products SET color = ? WHERE id = ?")
                .bind(pick)
                .bind(id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_colours_are_valid_and_distinct() {
        let set: HashSet<&str> = PALETTE.iter().copied().collect();
        assert_eq!(set.len(), PALETTE.len());
        assert!(PALETTE.iter().all(|c| is_hex_color(c)));
    }

    #[test]
    fn accepts_only_six_digit_hex() {
        assert_eq!(normal_color(" #e5484d "), Some("#E5484D".to_string()));
        for bad in [
            "", "E5484D", "#E5484", "#E5484DD", "#GGGGGG", "red", "#12345é",
        ] {
            assert_eq!(normal_color(bad), None, "{bad}");
        }
    }

    #[test]
    fn picks_a_colour_nobody_uses_and_never_runs_out() {
        let mut used = HashSet::new();
        for _ in 0..200 {
            let next = pick_color(&used);
            assert!(is_hex_color(&next), "{next}");
            assert!(used.insert(next), "a colour was handed out twice");
        }
    }
}
