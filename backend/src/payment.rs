use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::settings::PaySettings;

const ERROR_TEXT_MAX_CHARS: usize = 160;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePayment {
    pub id: String,
    pub status: String,
    pub qr_payload: Option<String>,
    pub bank: Option<String>,
    pub payer_name: Option<String>,
    /// Amount in satang as reported by the verify API.
    pub amount_satang: Option<i64>,
    /// Why a payment is still waiting, e.g. `kbank_amount_hidden`.
    pub verification_issue: Option<String>,
    /// When the QR stops being valid (RFC 3339). Only meaningful once the status is `pending`.
    pub expires_at: Option<String>,
}

/// An RFC 3339 timestamp from the API as the database's UTC text format.
pub fn db_time(rfc3339: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| t.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string())
}

#[derive(Deserialize)]
struct Envelope {
    data: RemotePayment,
}

#[derive(Deserialize)]
struct ListEnvelope<T> {
    data: Vec<T>,
}

#[derive(Deserialize)]
struct Listed {
    id: String,
    name: Option<String>,
    #[serde(default)]
    enabled: bool,
    /// Live state the API reports for a bot, e.g. `online`.
    status: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotState {
    Online,
    Offline,
    /// The API gave no status, or one this program does not know.
    Unknown,
}

pub struct BotHealth {
    pub state: BotState,
    pub name: Option<String>,
    pub status: Option<String>,
    pub enabled: bool,
}

fn classify_bot(enabled: bool, status: Option<&str>) -> BotState {
    if !enabled {
        return BotState::Offline;
    }
    match status.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("online" | "connected" | "active" | "running" | "ready" | "ok" | "healthy") => {
            BotState::Online
        }
        Some(
            "offline" | "disconnected" | "stopped" | "error" | "down" | "inactive" | "unavailable"
            | "failed",
        ) => BotState::Offline,
        _ => BotState::Unknown,
    }
}

/// A bot the verify API has, with the chats (sources of bank notifications) it reads; what the
/// settings page offers to pick from when more than one is enabled.
#[derive(Debug, Serialize)]
pub struct BotChoice {
    pub id: String,
    pub name: Option<String>,
    pub enabled: bool,
    pub status: Option<String>,
    pub targets: Vec<TargetChoice>,
}

#[derive(Debug, Serialize)]
pub struct TargetChoice {
    pub id: String,
    pub name: Option<String>,
    pub enabled: bool,
}

/// The bot and the chat the verify API reads bank notifications from.
#[derive(Debug, Clone)]
pub struct Route {
    pub bot_id: String,
    pub target_id: String,
    pub bot_name: Option<String>,
    pub target_name: Option<String>,
}

pub struct PingStatus {
    pub with_key: u16,
    pub without_key: u16,
}

#[derive(Clone)]
pub struct PaymentClient {
    http: reqwest::Client,
    cfg: PaySettings,
    id_type: &'static str,
    route: Arc<OnceCell<Route>>,
}

pub fn format_baht(satang: i64) -> String {
    format!("{}.{:02}", satang / 100, satang % 100)
}

/// Picks the only enabled entry; `what` names it in the error shown to the cashier.
fn only_enabled(items: Vec<Listed>, what: &str) -> Result<Listed> {
    let mut enabled: Vec<Listed> = items.into_iter().filter(|i| i.enabled).collect();
    match enabled.len() {
        1 => Ok(enabled.remove(0)),
        0 => bail!("ไม่พบ{what}ที่เปิดใช้งานใน API"),
        n => bail!("พบ{what}ที่เปิดใช้งาน {n} รายการ กรุณาระบุเองในหน้าตั้งค่า"),
    }
}

/// Turns a non-2xx response into an error carrying the API's own message.
async fn check(res: reqwest::Response) -> Result<reqwest::Response> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let body: Value = res.json().await.unwrap_or(Value::Null);
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(|m| m.chars().take(ERROR_TEXT_MAX_CHARS).collect::<String>());
    match message {
        Some(m) => bail!("API ตอบ HTTP {}: {m}", status.as_u16()),
        None => bail!("API ตอบ HTTP {}", status.as_u16()),
    }
}

impl PaymentClient {
    /// `None` when the settings are not complete enough to talk to the verify API.
    pub fn from_settings(cfg: &PaySettings) -> Result<Option<Self>> {
        let Some(id_type) = cfg.id_type().filter(|_| cfg.verify_ready()) else {
            return Ok(None);
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("build http client")?;
        Ok(Some(Self {
            http,
            cfg: cfg.clone(),
            id_type,
            route: Arc::default(),
        }))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.cfg.base_url.trim_end_matches('/'))
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let res = request
            .bearer_auth(&self.cfg.api_key)
            .send()
            .await
            .context("ติดต่อ API ไม่ได้ ตรวจสอบที่อยู่ API และอินเทอร์เน็ต")?;
        check(res).await
    }

    async fn list(&self, path: &str) -> Result<Vec<Listed>> {
        let res = self.send(self.http.get(self.url(path))).await?;
        Ok(res
            .json::<ListEnvelope<Listed>>()
            .await
            .context("API ส่งรายการกลับมาในรูปแบบที่อ่านไม่ได้")?
            .data)
    }

    async fn discover(&self) -> Result<Route> {
        let (bot_id, bot_name) = if self.cfg.bot_id.is_empty() {
            let bot = only_enabled(self.list("/v1/bots").await?, "บอท")?;
            (bot.id, bot.name)
        } else {
            (self.cfg.bot_id.clone(), None)
        };
        let (target_id, target_name) = if self.cfg.source_target_id.is_empty() {
            let path = format!("/v1/bots/{bot_id}/targets");
            let target = only_enabled(self.list(&path).await?, "แหล่งแจ้งเงินเข้า")?;
            (target.id, target.name)
        } else {
            (self.cfg.source_target_id.clone(), None)
        };
        Ok(Route {
            bot_id,
            target_id,
            bot_name,
            target_name,
        })
    }

    /// Bot and source target: the ones typed in settings, otherwise found through the API
    /// (when exactly one is enabled). A successful lookup is remembered.
    pub async fn route(&self) -> Result<&Route> {
        self.route.get_or_try_init(|| self.discover()).await
    }

    /// Every bot the API has with its chats, so the cashier can choose instead of typing IDs.
    pub async fn bots_with_targets(&self) -> Result<Vec<BotChoice>> {
        let mut out = Vec::new();
        for bot in self.list("/v1/bots").await? {
            let targets = self
                .list(&format!("/v1/bots/{}/targets", bot.id))
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| TargetChoice {
                    id: t.id,
                    name: t.name,
                    enabled: t.enabled,
                })
                .collect();
            out.push(BotChoice {
                id: bot.id,
                name: bot.name,
                enabled: bot.enabled,
                status: bot.status,
                targets,
            });
        }
        Ok(out)
    }

    /// Whether the bot that reads bank notifications is running, as the API reports it.
    pub async fn bot_health(&self) -> Result<BotHealth> {
        let bots = self.list("/v1/bots").await?;
        let bot = if self.cfg.bot_id.is_empty() {
            only_enabled(bots, "บอท")?
        } else {
            bots.into_iter()
                .find(|b| b.id == self.cfg.bot_id)
                .context("ไม่พบบอทตาม Bot ID ที่ตั้งไว้ใน API")?
        };
        Ok(BotHealth {
            state: classify_bot(bot.enabled, bot.status.as_deref()),
            name: bot.name,
            status: bot.status,
            enabled: bot.enabled,
        })
    }

    /// Looks up a made-up payment id twice, with and without the key, and returns both HTTP
    /// statuses. Comparing them shows whether the server really checks the key.
    pub async fn ping(&self) -> Result<PingStatus> {
        let url = self.url("/v1/promptpay/payments/connection-test");
        let reach = |req: reqwest::RequestBuilder| async move {
            req.send()
                .await
                .map(|r| r.status().as_u16())
                .context("ติดต่อ API ไม่ได้ ตรวจสอบที่อยู่ API และอินเทอร์เน็ต")
        };
        let with_key = reach(self.http.get(&url).bearer_auth(&self.cfg.api_key)).await?;
        let without_key = reach(self.http.get(&url)).await?;
        Ok(PingStatus {
            with_key,
            without_key,
        })
    }

    pub async fn create(
        &self,
        amount_satang: i64,
        idempotency_key: &str,
        expires_secs: u32,
    ) -> Result<RemotePayment> {
        let route = self.route().await?;
        let mut body = json!({
            "botId": route.bot_id,
            "sourceTargetId": route.target_id,
            "promptPayIdType": self.id_type,
            "promptPayId": self.cfg.id_digits(),
            "amount": format_baht(amount_satang),
            "expiresInSeconds": expires_secs,
        });
        if !self.cfg.dest_last4.is_empty() {
            body["destinationLast4"] = json!(self.cfg.dest_last4);
        }
        let res = self
            .send(
                self.http
                    .post(self.url("/v1/promptpay/payments"))
                    .header("Idempotency-Key", idempotency_key)
                    .json(&body),
            )
            .await?;
        Ok(res
            .json::<Envelope>()
            .await
            .context("API ส่งผลสร้างรายการกลับมาในรูปแบบที่อ่านไม่ได้")?
            .data)
    }

    /// Cancels a waiting payment so it stops holding a place in the API's queue.
    pub async fn cancel(&self, id: &str) -> Result<()> {
        let url = self.url(&format!("/v1/promptpay/payments/{id}/cancel"));
        self.send(self.http.post(url)).await.map(|_| ())
    }

    pub async fn get(&self, id: &str) -> Result<RemotePayment> {
        let res = self
            .send(
                self.http
                    .get(self.url(&format!("/v1/promptpay/payments/{id}"))),
            )
            .await?;
        Ok(res
            .json::<Envelope>()
            .await
            .context("API ส่งสถานะรายการกลับมาในรูปแบบที่อ่านไม่ได้")?
            .data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(id: &str, enabled: bool) -> Listed {
        Listed {
            id: id.into(),
            name: None,
            enabled,
            status: None,
        }
    }

    #[test]
    fn classifies_bot_status() {
        assert_eq!(classify_bot(true, Some("online")), BotState::Online);
        assert_eq!(classify_bot(true, Some(" Online ")), BotState::Online);
        assert_eq!(classify_bot(true, Some("offline")), BotState::Offline);
        assert_eq!(classify_bot(false, Some("online")), BotState::Offline);
        assert_eq!(classify_bot(true, None), BotState::Unknown);
        assert_eq!(classify_bot(true, Some("starting")), BotState::Unknown);
    }

    #[test]
    fn formats_baht_with_two_decimals() {
        assert_eq!(format_baht(5_500), "55.00");
        assert_eq!(format_baht(5_505), "55.05");
        assert_eq!(format_baht(99), "0.99");
    }

    #[test]
    fn converts_api_timestamps_to_database_time() {
        assert_eq!(
            db_time("2026-11-22T09:01:00.000Z").as_deref(),
            Some("2026-11-22 09:01:00")
        );
        assert_eq!(
            db_time("2026-11-22T16:01:00+07:00").as_deref(),
            Some("2026-11-22 09:01:00")
        );
        assert_eq!(db_time("not a date"), None);
    }

    #[test]
    fn parses_remote_payment() {
        let raw = r#"{"data":{"id":"p1","status":"queued","qrPayload":"000201","bank":null,"payerName":null,"extra":1}}"#;
        let parsed: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.data.status, "queued");
        assert_eq!(parsed.data.qr_payload.as_deref(), Some("000201"));
    }

    #[test]
    fn parses_bot_and_target_lists() {
        let raw = r#"{"data":[{"id":"bot_1","name":"kao","enabled":true,"status":"online"},{"id":"bot_2","enabled":false}],"meta":{"total":2}}"#;
        let parsed: ListEnvelope<Listed> = serde_json::from_str(raw).unwrap();
        let bot = only_enabled(parsed.data, "บอท").unwrap();
        assert_eq!(bot.id, "bot_1");
        assert_eq!(bot.name.as_deref(), Some("kao"));
    }

    #[test]
    fn picks_only_a_single_enabled_entry() {
        assert!(only_enabled(vec![listed("a", false), listed("b", true)], "x").is_ok());
        assert!(only_enabled(vec![listed("a", false)], "x").is_err());
        assert!(only_enabled(vec![listed("a", true), listed("b", true)], "x").is_err());
        assert!(only_enabled(Vec::new(), "x").is_err());
    }
}
