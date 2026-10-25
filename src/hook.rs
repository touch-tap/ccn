//! The hook dispatcher. Claude Code runs `ccn hook` with the event JSON on stdin; this decides
//! what to do and prints a decision when it has one. It never exits non-zero: a failure here
//! must not block Claude.

use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::api::{Api, Wait};
use crate::config::{self, Config};
use crate::logger::log;
use crate::payload::{self, Input};
use crate::session::{self, Card, SessionState};
use crate::{notify, presence, respond};

const EVENT_TIMEOUT: Duration = Duration::from_secs(3);
const QUICK_TIMEOUT: Duration = Duration::from_secs(2);
const SETTINGS_MAX_AGE: i64 = 60 * 60;
const REGISTER_MAX_AGE: i64 = 24 * 60 * 60;

pub fn run() -> Result<()> {
    match std::panic::catch_unwind(run_inner) {
        Ok(Ok(Some(output))) => println!("{output}"),
        Ok(Ok(None)) => {}
        Ok(Err(err)) => log(&format!("hook error: {err:#}")),
        Err(_) => log("hook panicked"),
    }
    Ok(())
}

struct Ctx {
    cfg: Config,
    api: Api,
    input: Value,
    state: SessionState,
}

fn run_inner() -> Result<Option<Value>> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let input: Value = serde_json::from_str(&raw).context("hook input is not JSON")?;
    let event = input["hook_event_name"].as_str().unwrap_or("").to_string();
    let session_id = Input(&input).session_id();

    let cfg = Config::load();
    if !cfg.is_paired() {
        return Ok(None);
    }
    let api = Api::new(&cfg);
    let mut state = SessionState::load(&session_id);
    state.touch(&input);
    let mut ctx = Ctx { cfg, api, input, state };

    match event.as_str() {
        "SessionStart" => ctx.session_start(),
        "UserPromptSubmit" => ctx.user_prompt(),
        "PreToolUse" => ctx.pre_tool_use(),
        "PermissionRequest" => ctx.permission_request(),
        "Notification" => ctx.notification(),
        "Stop" => ctx.stop(),
        "SessionEnd" => ctx.session_end(),
        other => {
            log(&format!("ignoring event {other}"));
            Ok(None)
        }
    }
}

impl Ctx {
    fn input(&self) -> Input<'_> {
        Input(&self.input)
    }

    fn quiet(&self) -> bool {
        self.cfg.settings.in_quiet_hours(chrono::Local::now().time())
    }

    // ---- events ----

    fn session_start(&mut self) -> Result<Option<Value>> {
        self.state.started_at = config::now();
        self.state.source = self.input().str("source").map(|s| s.to_string());
        self.state.save()?;
        session::prune_old(7);

        let now = config::now();
        if now - self.cfg.settings_fetched_at.unwrap_or(0) > SETTINGS_MAX_AGE {
            match self.api.get_settings(self.cfg.settings_etag.as_deref(), QUICK_TIMEOUT) {
                Ok(Some(result)) => {
                    self.cfg.settings = result.settings;
                    self.cfg.settings_updated_at = result.updated_at;
                    self.cfg.settings_etag = result.etag;
                    self.cfg.settings_fetched_at = Some(now);
                    let _ = self.cfg.save();
                }
                Ok(None) => {
                    self.cfg.settings_fetched_at = Some(now);
                    let _ = self.cfg.save();
                }
                Err(err) => log(&format!("settings sync skipped: {err:#}")),
            }
        }
        if now - self.cfg.registered_at.unwrap_or(0) > REGISTER_MAX_AGE {
            if let Some(identity) = crate::commands::identity(&self.cfg) {
                match self.api.register(&identity) {
                    Ok(()) => {
                        self.cfg.registered_at = Some(now);
                        let _ = self.cfg.save();
                    }
                    Err(err) => log(&format!("re-registration skipped: {err:#}")),
                }
            }
        }
        Ok(None)
    }

    fn user_prompt(&mut self) -> Result<Option<Value>> {
        self.settle_last_card();
        if let Some(prompt) = self.input().str("prompt") {
            self.state.prompt = Some(SessionState::truncate(prompt, 4000));
            self.state.prompt_at = Some(config::now());
        }
        self.state.save()?;
        Ok(None)
    }

    fn pre_tool_use(&mut self) -> Result<Option<Value>> {
        self.settle_last_card();
        let tool_name = self.input().tool_name();
        self.state.tool_name = tool_name.clone();
        self.state.tool_input = Some(self.input().tool_input());
        self.state.tool_at = Some(config::now());
        self.state.save()?;

        if tool_name.as_deref() == Some("AskUserQuestion") && self.cfg.settings.notify_on_question() {
            let tool_input = self.input().tool_input();
            let questions = payload::questions_from_input(&tool_input);
            if questions.is_empty() {
                return Ok(None);
            }
            let (card, key) = payload::question_card(&self.input(), &questions, false);
            let ceiling = self.cfg.settings.hold_ceiling();
            let answer = self.blocking("question", &key, card, ceiling, true)?;
            return Ok(answer.and_then(|text| respond::question_output(&text, &tool_input, &questions)));
        }
        Ok(None)
    }

    fn permission_request(&mut self) -> Result<Option<Value>> {
        if !self.cfg.settings.notify_on_permission() {
            return Ok(None);
        }
        let (card, kind, key) = payload::permission_card(&self.input(), false);
        let ceiling = self.cfg.settings.hold_ceiling();
        // When the user is present we send nothing here: Claude Code shows its prompt and the
        // Notification hook sends the informational card.
        let answer = self.blocking(kind, &key, card, ceiling, false)?;
        Ok(answer.map(|text| respond::permission_output(&text)))
    }

    fn notification(&mut self) -> Result<Option<Value>> {
        let kind = self.input().str("notification_type").unwrap_or("").to_string();
        if kind != "permission_prompt" || !self.cfg.settings.notify_on_permission() || self.quiet() {
            return Ok(None);
        }
        // A card for this request may already be on the phone from the PermissionRequest hook
        if let Some(card) = &self.state.last_card {
            let fresh = config::now() - card.sent_at < self.cfg.settings.hold_ceiling() as i64 + 60;
            if fresh && !card.settled && (card.kind == "permission" || card.kind == "plan" || card.kind == "question") {
                return Ok(None);
            }
        }
        let (card, kind, key) = if self.state.tool_name.is_some() && self.input().tool_name().is_none() {
            // The Notification event carries no tool; use what PreToolUse recorded
            let mut synthetic = self.input.clone();
            synthetic["tool_name"] = Value::String(self.state.tool_name.clone().unwrap_or_default());
            synthetic["tool_input"] = self.state.tool_input.clone().unwrap_or(Value::Null);
            payload::permission_card(&Input(&synthetic), false)
        } else {
            payload::permission_card(&self.input(), false)
        };
        self.send_card(kind, &key, card, false)?;
        Ok(None)
    }

    fn stop(&mut self) -> Result<Option<Value>> {
        if !self.cfg.settings.notify_on_done() {
            return Ok(None);
        }
        let stop_hook_active = self.input["stop_hook_active"].as_bool().unwrap_or(false);
        if stop_hook_active {
            if let Some(card) = &self.state.last_card {
                if card.kind == "done" && config::now() - card.sent_at < 5 {
                    return Ok(None);
                }
            }
        }
        self.settle_last_card();
        let card = payload::done_card(&self.input(), &self.state, false);
        let hold = self.cfg.settings.stop_hold();
        let answer = self.blocking("done", "done", card, hold, true)?;
        Ok(answer.and_then(|text| respond::stop_output(&text)))
    }

    fn session_end(&mut self) -> Result<Option<Value>> {
        let session_id = self.input().session_id();
        let _ = self.api.post_event(&payload::session_update(&session_id, "ended"), QUICK_TIMEOUT);
        SessionState::remove(&session_id);
        Ok(None)
    }

    // ---- helpers ----

    /// Tells the server the previous request in this session was answered at the keyboard.
    fn settle_last_card(&mut self) {
        let Some(card) = &mut self.state.last_card else { return };
        if card.settled {
            return;
        }
        let update = payload::session_update(&self.state.session_id, "responded");
        match self.api.post_event(&update, QUICK_TIMEOUT) {
            Ok(_) => card.settled = true,
            Err(err) => log(&format!("could not settle card: {err:#}")),
        }
        let _ = self.state.save();
    }

    fn send_card(&mut self, kind: &str, key: &str, mut card: Value, held: bool) -> Result<Option<String>> {
        card["terminalContext"]["supportsRemoteInput"] = Value::Bool(held);
        let id = self.api.post_event(&card, EVENT_TIMEOUT)?;
        if let Some(id) = &id {
            self.state.last_card = Some(Card {
                notification_id: id.clone(),
                kind: kind.to_string(),
                key: key.to_string(),
                sent_at: config::now(),
                held,
                settled: false,
            });
            let _ = self.state.save();
        }
        if self.cfg.desktop_notifications {
            let title = card["title"].as_str().unwrap_or("Claude Code");
            let body = card["promptText"].as_str().or(card["message"].as_str()).unwrap_or("Needs your attention");
            notify::show(title, body, self.cfg.settings.sound());
        }
        Ok(id)
    }

    /// The presence rule. Returns the phone's response text when one arrived while holding.
    fn blocking(
        &mut self,
        kind: &str,
        key: &str,
        card: Value,
        ceiling: u64,
        present_sends_info: bool,
    ) -> Result<Option<String>> {
        if self.quiet() {
            return Ok(None);
        }
        let threshold = self.cfg.settings.presence_threshold();
        if ceiling == 0 || presence::user_present(threshold) {
            if present_sends_info {
                self.send_card(kind, key, card, false)?;
            }
            return Ok(None);
        }

        let Some(id) = self.send_card(kind, key, card, true)? else { return Ok(None) };
        log(&format!("holding {kind} {id} for up to {ceiling}s"));

        let deadline = Instant::now() + Duration::from_secs(ceiling);
        let (tx, rx) = mpsc::channel::<Option<String>>();
        let api = self.api.clone();
        let poll_id = id.clone();
        std::thread::spawn(move || {
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
                if remaining == 0 {
                    let _ = tx.send(None);
                    return;
                }
                match api.wait_response(&poll_id, remaining.clamp(1, 25)) {
                    Ok(Wait::Responded(text)) => {
                        let _ = tx.send(Some(text));
                        return;
                    }
                    Ok(Wait::Settled(status)) => {
                        log(&format!("{poll_id} settled as {status}"));
                        let _ = tx.send(None);
                        return;
                    }
                    Ok(Wait::Pending) => {}
                    Err(err) => {
                        log(&format!("wait error: {err:#}"));
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        });

        loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Some(text)) => {
                    if let Some(card) = &mut self.state.last_card {
                        card.settled = true;
                    }
                    let _ = self.state.save();
                    log(&format!("{id} answered from the phone"));
                    return Ok(Some(text));
                }
                Ok(None) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if presence::user_present(threshold) {
                        log(&format!("{id}: user returned to the keyboard"));
                        return Ok(None);
                    }
                    if Instant::now() >= deadline {
                        log(&format!("{id}: hold ceiling reached"));
                        return Ok(None);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}
