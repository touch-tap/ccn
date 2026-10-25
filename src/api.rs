//! The server over HTTPS. Every call has a short timeout: a hook must never hang Claude Code.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use crate::config::{self, Config, Settings};

#[derive(Clone)]
pub struct Api {
    base: String,
    token: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ClaimResult {
    pub pairing_id: String,
    pub device: ClaimedDevice,
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
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CodeResponse {
    pub code: String,
    pub expires_in: u64,
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
        Api { base: cfg.server_url.trim_end_matches('/').to_string(), token: config::load_token(cfg) }
    }

    pub fn with_token(cfg: &Config, token: String) -> Api {
        Api { base: cfg.server_url.trim_end_matches('/').to_string(), token: Some(token) }
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
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
                let mut req = if method == "GET" { agent.get(&url) } else { agent.delete(&url) };
                if let Some(b) = &bearer {
                    req = req.header("Authorization", b);
                }
                for (k, v) in extra {
                    req = req.header(*k, *v);
                }
                req.call()
            }
            _ => {
                let mut req = if method == "POST" { agent.post(&url) } else { agent.put(&url) };
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
        let etag = response.headers().get("etag").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
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
        let reply = self.send("POST", "/api/pairing/claim-by-client", Some(body), QUICK, &[])?;
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

    pub fn pairings(&self) -> Result<Vec<PairedDevice>> {
        let reply = self.send("GET", "/api/pairings", None, QUICK, &[])?;
        let reply = Self::expect_ok(reply, "listing paired phones")?;
        #[derive(Deserialize)]
        struct Devices {
            devices: Vec<PairedDevice>,
        }
        Ok(serde_json::from_str::<Devices>(&reply.body).context("unexpected pairings response")?.devices)
    }

    pub fn revoke(&self, pairing_id: &str) -> Result<()> {
        let reply = self.send("DELETE", &format!("/api/pairings/{pairing_id}"), None, QUICK, &[])?;
        Self::expect_ok(reply, "revoking the pairing").map(|_| ())
    }

    pub fn health(&self) -> Result<()> {
        let reply = self.send("GET", "/health", None, Duration::from_secs(5), &[])?;
        Self::expect_ok(reply, "server health").map(|_| ())
    }

    // ---- settings ----

    /// `Ok(None)` means the server's copy matches `etag` (304).
    pub fn get_settings(&self, etag: Option<&str>, timeout: Duration) -> Result<Option<SettingsResult>> {
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
        let parsed: Body = serde_json::from_str(&reply.body).context("unexpected settings response")?;
        Ok(SettingsResult { settings: parsed.settings, updated_at: parsed.updated_at, etag: reply.etag })
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
                let v: Value = serde_json::from_str(&reply.body).context("unexpected response body")?;
                let status = v["status"].as_str().unwrap_or("").to_string();
                let source = v["responseSource"].as_str().unwrap_or("");
                match (status.as_str(), v["response"].as_str()) {
                    ("responded", Some(text)) if source == "mobile" => Ok(Wait::Responded(text.to_string())),
                    _ => Ok(Wait::Settled(status)),
                }
            }
            404 => Ok(Wait::Settled("gone".to_string())),
            _ => Err(anyhow!("waiting for a response: {}", Self::error_message(&reply))),
        }
    }
}
