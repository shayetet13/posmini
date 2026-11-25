use std::collections::HashMap;

use anyhow::Result;
use sqlx::SqlitePool;

/// Payment-related settings. Stored per shop in the `settings` table;
/// `.env` values are only the defaults for keys that were never saved.
#[derive(Clone, Debug, Default)]
pub struct PaySettings {
    pub base_url: String,
    pub api_key: String,
    pub promptpay_id: String,
    pub dest_last4: String,
    /// Optional: left blank, the bot is found through the API when exactly one is enabled.
    pub bot_id: String,
    /// Optional: left blank, the source chat is found through the API likewise.
    pub source_target_id: String,
}

const KEYS: [&str; 6] = [
    "pay_api_base",
    "pay_api_key",
    "promptpay_id",
    "pay_dest_last4",
    "pay_bot_id",
    "pay_source_target_id",
];

pub fn digits_of(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

impl PaySettings {
    pub fn id_digits(&self) -> String {
        digits_of(&self.promptpay_id)
    }

    /// PromptPay id type expected by the verify API, derived from the number's length.
    pub fn id_type(&self) -> Option<&'static str> {
        let digits = self.id_digits();
        match digits.len() {
            10 if digits.starts_with('0') => Some("phone"),
            13 => Some("national_id"),
            15 => Some("ewallet"),
            _ => None,
        }
    }

    pub fn verify_ready(&self) -> bool {
        let scheme_ok =
            self.base_url.starts_with("https://") || self.base_url.starts_with("http://");
        scheme_ok
            && !self.base_url.contains("your-api-host")
            && !self.api_key.is_empty()
            && self.id_type().is_some()
    }

    fn values(&self) -> [&str; 6] {
        [
            &self.base_url,
            &self.api_key,
            &self.promptpay_id,
            &self.dest_last4,
            &self.bot_id,
            &self.source_target_id,
        ]
    }
}

/// Every stored setting of the shop, by key.
pub async fn load_map(pool: &SqlitePool, shop_id: i64) -> Result<HashMap<String, String>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE shop_id = ?")
            .bind(shop_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

pub async fn save_pairs(pool: &SqlitePool, shop_id: i64, pairs: &[(&str, String)]) -> Result<()> {
    let mut tx = pool.begin().await?;
    for (key, value) in pairs {
        sqlx::query(
            "INSERT INTO settings (shop_id, key, value) VALUES (?, ?, ?) \
             ON CONFLICT(shop_id, key) DO UPDATE SET value = excluded.value",
        )
        .bind(shop_id)
        .bind(key)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn load(pool: &SqlitePool, shop_id: i64, defaults: &PaySettings) -> Result<PaySettings> {
    let stored = load_map(pool, shop_id).await?;
    let pick = |key: &str, default: &str| {
        stored
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    };
    Ok(PaySettings {
        base_url: pick(KEYS[0], &defaults.base_url),
        api_key: pick(KEYS[1], &defaults.api_key),
        promptpay_id: pick(KEYS[2], &defaults.promptpay_id),
        dest_last4: pick(KEYS[3], &defaults.dest_last4),
        bot_id: pick(KEYS[4], &defaults.bot_id),
        source_target_id: pick(KEYS[5], &defaults.source_target_id),
    })
}

pub async fn save(pool: &SqlitePool, shop_id: i64, settings: &PaySettings) -> Result<()> {
    let pairs: Vec<(&str, String)> = KEYS
        .iter()
        .copied()
        .zip(settings.values().map(str::to_string))
        .collect();
    save_pairs(pool, shop_id, &pairs).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_id(id: &str) -> PaySettings {
        PaySettings {
            promptpay_id: id.to_string(),
            ..PaySettings::default()
        }
    }

    #[test]
    fn derives_id_type_from_digits() {
        assert_eq!(with_id("081-234-5678").id_type(), Some("phone"));
        assert_eq!(with_id("1234567890123").id_type(), Some("national_id"));
        assert_eq!(with_id("123456789012345").id_type(), Some("ewallet"));
        assert_eq!(with_id("1234567890").id_type(), None);
        assert_eq!(with_id("").id_type(), None);
    }

    #[test]
    fn verify_needs_url_key_and_promptpay_id() {
        let mut s = PaySettings {
            base_url: "https://api.example.com".into(),
            api_key: "k".into(),
            promptpay_id: "0812345678".into(),
            dest_last4: String::new(),
            bot_id: String::new(),
            source_target_id: String::new(),
        };
        assert!(s.verify_ready());
        s.api_key.clear();
        assert!(!s.verify_ready());
        s.api_key = "k".into();
        s.base_url = "https://your-api-host".into();
        assert!(!s.verify_ready());
    }
}
