//! The server over HTTPS. Every call has a short timeout: a hook must never hang Claude Code.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::config::{self, Config, Settings};

#[derive(Clone)]
pub struct Api {
    base: String,
    token: Option<String>,
    /// Set when the server refused this computer's credentials on any call, including ones whose
    /// errors the caller only logs.
    rejected: Arc<AtomicBool>,
}

/// The server no longer accepts this computer's credentials: its record (and every pairing) is
/// gone, for example after 60 days without use. Only `ccn pair` fixes it, so nothing retries.
#[derive(Debug)]
pub struct Rejected;

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this computer isn't paired any more (the server no longer recognises it). Run `ccn pair` to reconnect")
    }
}

impl std::error::Error for Rejected {}

pub fn is_rejected(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Rejected>().is_some()
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ClaimResult {
    pub pairing_id: String,
    pub device: ClaimedDevice,
    /// The answerable welcome the server pushed to the phone (servers before Oct 2026 don't send it)
    #[serde(default)]
    pub welcome_notification_id: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ClaimedDevice {
    pub id: String,
    pub name: Option<String>,
    pub platform: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub platform: Option<String>,
    pub client_kind: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub settings_updated_at: Option<i64>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub id: String,
    pub pairing_id: String,
    pub name: Option<String>,
    pub platform: String,
    pub created_at: i64,
    pub notifications_enabled: Option<bool>,
    #[serde(default)]
    pub welcome_notification_id: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CodeResponse {
    pub code: String,
    pub expires_in: u64,
    /// claudecodenotifier.com/c/<token>: what the QR code opens on the phone
    #[serde(default)]
    pub link_url: Option<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct SettingsResult {
    pub settings: Settings,
    pub updated_at: Option<i64>,
    pub etag: Option<String>,
}

/// What a long-poll for the phone's answer came back with.
#[derive(Debug)]
pub enum Wait {
    /// The phone answered with this response string.
    Responded(String),
    /// The request was settled some other way (dismissed, expired, answered locally).
    Settled(String),
    /// Nothing yet.
    Pending,
}

struct Reply {
    status: u16,
    body: String,
    etag: Option<String>,
}

const QUICK: Duration = Duration::from_secs(10);

impl Api {
    pub fn new(cfg: &Config) -> Api {
        Api::build(cfg, config::load_token(cfg))
    }

    pub fn with_token(cfg: &Config, token: String) -> Api {
        Api::build(cfg, Some(token))
    }

    fn build(cfg: &Config, token: Option<String>) -> Api {
        Api {
            base: cfg.server_url.trim_end_matches('/').to_string(),
            token,
            rejected: Arc::default(),
        }
    }

    pub fn was_rejected(&self) -> bool {
        self.rejected.load(Ordering::Relaxed)
    }

    fn agent(timeout: Duration) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .new_agent()
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
        extra: &[(&str, &str)],
    ) -> Result<Reply> {
        let url = format!("{}{}", self.base, path);
        let agent = Self::agent(timeout);
        let bearer = self.token.as_ref().map(|t| format!("Bearer {t}"));

        let mut response = match method {
            "GET" | "DELETE" => {
                let mut req = if method == "GET" {
                    agent.get(&url)
                } else {
                    agent.delete(&url)
                };
                if let Some(b) = &bearer {
                    req = req.header("Authorization", b);
                }
                for (k, v) in extra {
                    req = req.header(*k, *v);
                }
                req.call()
            }
            _ => {
                let mut req = if method == "POST" {
                    agent.post(&url)
                } else {
                    agent.put(&url)
                };
                if let Some(b) = &bearer {
                    req = req.header("Authorization", b);
                }
                for (k, v) in extra {
                    req = req.header(*k, *v);
                }
                req.send_json(body.cloned().unwrap_or_else(|| json!({})))
            }
        }
        .with_context(|| format!("{method} {path}"))?;

        let status = response.status().as_u16();
        if (status == 401 || status == 403) && self.token.is_some() {
            self.rejected.store(true, Ordering::Relaxed);
            return Err(anyhow::Error::new(Rejected));
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = response.body_mut().read_to_string().unwrap_or_default();
        Ok(Reply { status, body, etag })
    }

    fn error_message(reply: &Reply) -> String {
        serde_json::from_str::<Value>(&reply.body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| format!("server returned {}", reply.status))
    }

    fn expect_ok(reply: Reply, what: &str) -> Result<Reply> {
        if (200..300).contains(&reply.status) {
            Ok(reply)
        } else {
            Err(anyhow!("{what}: {}", Self::error_message(&reply)))
        }
    }

    // ---- registration and pairing ----

    pub fn register(&self, identity: &Value) -> Result<()> {
        let reply = self.send("POST", "/api/clients/register", Some(identity), QUICK, &[])?;
        Self::expect_ok(reply, "registering this computer").map(|_| ())
    }

    pub fn claim_by_client(&self, body: &Value) -> Result<ClaimResult> {
        let reply = self.send(
            "POST",
            "/api/pairing/claim-by-client",
            Some(body),
            QUICK,
            &[],
        )?;
        let reply = Self::expect_ok(reply, "pairing")?;
        serde_json::from_str(&reply.body).context("unexpected pairing response")
    }

    pub fn request_code(&self) -> Result<CodeResponse> {
        let reply = self.send("POST", "/api/pairing/request", None, QUICK, &[])?;
        let reply = Self::expect_ok(reply, "requesting a pairing code")?;
        serde_json::from_str(&reply.body).context("unexpected code response")
    }

    pub fn me(&self) -> Result<Me> {
        let reply = self.send("GET", "/api/clients/me", None, QUICK, &[])?;
        let reply = Self::expect_ok(reply, "reading this computer's record")?;
        serde_json::from_str(&reply.body).context("unexpected /me response")
    }

    /// Erases this computer from the server: its record, pairings, queued notifications and
    /// licence-slot name. A computer the server doesn't know comes back as `Rejected`.
    pub fn erase(&self) -> Result<()> {
        let reply = self.send("DELETE", "/api/clients/me", None, QUICK, &[])?;
        Self::expect_ok(reply, "erasing this computer").map(|_| ())
    }

    pub fn pairings(&self) -> Result<Vec<PairedDevice>> {
        let reply = self.send("GET", "/api/pairings", None, QUICK, &[])?;
        let reply = Self::expect_ok(reply, "listing paired phones")?;
        #[derive(Deserialize)]
        struct Devices {
            devices: Vec<PairedDevice>,
        }
        Ok(serde_json::from_str::<Devices>(&reply.body)
            .context("unexpected pairings response")?
            .devices)
    }

    pub fn revoke(&self, pairing_id: &str) -> Result<()> {
        let reply = self.send(
            "DELETE",
            &format!("/api/pairings/{pairing_id}"),
            None,
            QUICK,
            &[],
        )?;
        Self::expect_ok(reply, "revoking the pairing").map(|_| ())
    }

    pub fn health(&self) -> Result<()> {
        let reply = self.send("GET", "/health", None, Duration::from_secs(5), &[])?;
        Self::expect_ok(reply, "server health").map(|_| ())
    }

    // ---- settings ----

    /// `Ok(None)` means the server's copy matches `etag` (304).
    pub fn get_settings(
        &self,
        etag: Option<&str>,
        timeout: Duration,
    ) -> Result<Option<SettingsResult>> {
        let extra: Vec<(&str, &str)> = etag.map(|e| vec![("If-None-Match", e)]).unwrap_or_default();
        let reply = self.send("GET", "/api/clients/me/settings", None, timeout, &extra)?;
        if reply.status == 304 {
            return Ok(None);
        }
        let reply = Self::expect_ok(reply, "reading settings")?;
        Self::parse_settings(reply).map(Some)
    }

    pub fn put_settings(&self, settings: &Settings) -> Result<SettingsResult> {
        let body = json!({ "settings": settings });
        let reply = self.send("PUT", "/api/clients/me/settings", Some(&body), QUICK, &[])?;
        let reply = Self::expect_ok(reply, "saving settings")?;
        Self::parse_settings(reply)
    }

    fn parse_settings(reply: Reply) -> Result<SettingsResult> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            settings: Settings,
            updated_at: Option<i64>,
        }
        let parsed: Body =
            serde_json::from_str(&reply.body).context("unexpected settings response")?;
        Ok(SettingsResult {
            settings: parsed.settings,
            updated_at: parsed.updated_at,
            etag: reply.etag,
        })
    }

    // ---- hook events ----

    /// Posts one event. Returns the stored notification id for a `notification` event.
    pub fn post_event(&self, event: &Value, timeout: Duration) -> Result<Option<String>> {
        let reply = self.send("POST", "/api/clients/me/events", Some(event), timeout, &[])?;
        let reply = Self::expect_ok(reply, "sending the event")?;
        Ok(serde_json::from_str::<Value>(&reply.body)
            .ok()
            .and_then(|v| v["notificationId"].as_str().map(|s| s.to_string())))
    }

    /// Long-polls for the phone's answer, up to `wait_secs` (max 25).
    pub fn wait_response(&self, notification_id: &str, wait_secs: u64) -> Result<Wait> {
        let wait = wait_secs.min(25);
        let path = format!("/api/clients/me/notifications/{notification_id}/response?wait={wait}");
        let reply = self.send("GET", &path, None, Duration::from_secs(wait + 10), &[])?;
        match reply.status {
            204 => Ok(Wait::Pending),
            200 => {
                let v: Value =
                    serde_json::from_str(&reply.body).context("unexpected response body")?;
                let status = v["status"].as_str().unwrap_or("").to_string();
                let source = v["responseSource"].as_str().unwrap_or("");
                match (status.as_str(), v["response"].as_str()) {
                    ("responded", Some(text)) if source == "mobile" => {
                        Ok(Wait::Responded(text.to_string()))
                    }
                    _ => Ok(Wait::Settled(status)),
                }
            }
            404 => Ok(Wait::Settled("gone".to_string())),
            _ => Err(anyhow!(
                "waiting for a response: {}",
                Self::error_message(&reply)
            )),
        }
    }
}

// ============================================
// Licences
// ============================================

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Licence {
    pub plan: String,
    pub status: String,
    /// When an annual licence renews or ends (unix seconds); None for lifetime
    pub valid_until: Option<i64>,
    pub key_last4: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LicenceComputer {
    /// The slot, which is what `--replace` frees
    pub id: String,
    pub name: Option<String>,
    #[allow(dead_code)]
    pub platform: Option<String>,
    #[allow(dead_code)]
    pub activated_at: i64,
    pub last_seen_at: Option<i64>,
    #[serde(default)]
    pub current: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LicenceStatus {
    pub licensed: bool,
    pub licence: Option<Licence>,
    #[serde(default)]
    pub computers: Vec<LicenceComputer>,
    /// Show the once-a-day upgrade line in Claude Code (the server decides; off until launch)
    #[serde(default)]
    pub nudge: bool,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Checkout {
    pub url: String,
    pub session_id: String,
}

/// What a long-poll on a checkout came back with.
#[derive(Debug)]
pub enum CheckoutWait {
    Pending,
    Complete {
        /// The new key, the first time it is asked for; None after that, and for an upgrade
        key: Option<String>,
        /// This computer holds one of the licence's slots
        activated: bool,
        upgraded: bool,
        licence: Licence,
    },
}

/// A licence call the server refused, with its code (`SLOTS_FULL`, `INVALID_KEY`, …) and the whole
/// body, which for some codes lists the licence's computers.
#[derive(Debug)]
pub struct LicenceError {
    pub code: String,
    pub message: String,
    pub body: Value,
}

impl std::fmt::Display for LicenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LicenceError {}

impl LicenceError {
    pub fn computers(&self) -> Vec<LicenceComputer> {
        serde_json::from_value(self.body["computers"].clone()).unwrap_or_default()
    }
    pub fn licence(&self) -> Option<Licence> {
        serde_json::from_value(self.body["licence"].clone()).ok()
    }
}

impl Api {
    fn licence_reply(reply: Reply, what: &str) -> Result<Value> {
        let body: Value = serde_json::from_str(&reply.body).unwrap_or(Value::Null);
        if (200..300).contains(&reply.status) {
            return Ok(body);
        }
        let code = body["error"]["code"].as_str().unwrap_or("").to_string();
        let message = body["error"]["message"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{what}: server returned {}", reply.status));
        Err(anyhow::Error::new(LicenceError {
            code,
            message,
            body,
        }))
    }

    pub fn start_checkout(&self, plan: &str, upgrade: bool) -> Result<Checkout> {
        let body = json!({ "plan": plan, "upgrade": upgrade });
        let reply = self.send("POST", "/api/clients/me/checkout", Some(&body), QUICK, &[])?;
        serde_json::from_value(Self::licence_reply(reply, "starting the checkout")?)
            .context("unexpected checkout response")
    }

    /// Long-polls the checkout for up to `wait_secs` (max 25).
    pub fn wait_checkout(&self, session_id: &str, wait_secs: u64) -> Result<CheckoutWait> {
        let wait = wait_secs.min(25);
        let path = format!("/api/clients/me/checkout/{session_id}?wait={wait}");
        let reply = self.send("GET", &path, None, Duration::from_secs(wait + 10), &[])?;
        let v = Self::licence_reply(reply, "waiting for the payment")?;
        if v["status"] != "complete" {
            return Ok(CheckoutWait::Pending);
        }
        Ok(CheckoutWait::Complete {
            key: v["key"].as_str().map(|s| s.to_string()),
            activated: v["activated"].as_bool().unwrap_or(false),
            upgraded: v["upgraded"].as_bool().unwrap_or(false),
            licence: serde_json::from_value(v["licence"].clone())
                .context("unexpected checkout response")?,
        })
    }

    pub fn licence_status(&self, timeout: Duration) -> Result<LicenceStatus> {
        let reply = self.send("GET", "/api/clients/me/licence", None, timeout, &[])?;
        serde_json::from_value(Self::licence_reply(reply, "reading the licence")?)
            .context("unexpected licence response")
    }

    pub fn activate_licence(&self, key: &str, replace: Option<&str>) -> Result<LicenceStatus> {
        let body = json!({ "key": key, "replace": replace });
        let reply = self.send(
            "POST",
            "/api/clients/me/licence/activate",
            Some(&body),
            QUICK,
            &[],
        )?;
        serde_json::from_value(Self::licence_reply(reply, "activating the licence")?)
            .context("unexpected activation response")
    }

    /// True when this computer held a slot and now doesn't.
    pub fn deactivate_licence(&self) -> Result<bool> {
        let reply = self.send(
            "POST",
            "/api/clients/me/licence/deactivate",
            None,
            QUICK,
            &[],
        )?;
        Ok(
            Self::licence_reply(reply, "freeing the licence")?["deactivated"]
                .as_bool()
                .unwrap_or(false),
        )
    }
}

pub fn licence_error(err: &anyhow::Error) -> Option<&LicenceError> {
    err.downcast_ref::<LicenceError>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_is_recognised_through_context() {
        let err = Err::<(), _>(anyhow::Error::new(Rejected))
            .context("sending the event")
            .unwrap_err();
        assert!(is_rejected(&err));
        assert!(format!("{err:#}").contains("Run `ccn pair`"));
        assert!(!is_rejected(&anyhow!("server returned 500")));
    }
}
