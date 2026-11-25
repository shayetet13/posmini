use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::settings::PaySettings;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub shop_slug: String,
    pub shop_name: String,
    pub admin_user: String,
    pub admin_password: String,
    pub cookie_secure: bool,
    pub trust_proxy: bool,
    /// Serve the web pages too (`SERVE_UI`, default true). Off when the pages come from a separate frontend service.
    pub serve_ui: bool,
    /// Defaults for payment settings; the settings page (database) overrides them.
    pub pay_defaults: PaySettings,
    /// Developer option (`PRINT_DUMP_FILE`): append print jobs to this file instead of printing.
    pub print_dump: Option<PathBuf>,
    /// Optional Thai font file (`PRINT_FONT`) for image printing; Windows fonts are used otherwise.
    pub print_font: Option<PathBuf>,
}

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

fn required(name: &str) -> Result<String> {
    let value = var(name);
    if value.is_empty() {
        anyhow::bail!("{name} must be set in .env");
    }
    Ok(value)
}

fn pay_defaults() -> PaySettings {
    let base_url = var("PAYMENT_API_BASE").trim_end_matches('/').to_string();
    PaySettings {
        base_url: if base_url.contains("your-api-host") {
            String::new()
        } else {
            base_url
        },
        api_key: var("PROMPTPAY_API_KEY"),
        promptpay_id: var("PROMPTPAY_ID"),
        dest_last4: var("PAYMENT_DEST_LAST4"),
        bot_id: var("PAYMENT_BOT_ID"),
        source_target_id: var("PAYMENT_SOURCE_TARGET_ID"),
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: required("DATABASE_URL").context("database")?,
            bind_addr: required("BIND_ADDR")?,
            shop_slug: required("SHOP_SLUG")?,
            shop_name: required("SHOP_NAME")?,
            admin_user: required("ADMIN_USER")?,
            admin_password: required("ADMIN_PASSWORD")?,
            cookie_secure: var("COOKIE_SECURE") == "true",
            trust_proxy: var("TRUST_PROXY") == "true",
            serve_ui: var("SERVE_UI") != "false",
            pay_defaults: pay_defaults(),
            print_dump: Some(var("PRINT_DUMP_FILE"))
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            print_font: Some(var("PRINT_FONT"))
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
        })
    }
}
