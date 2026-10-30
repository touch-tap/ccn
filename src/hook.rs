//! The hook dispatcher. Claude Code runs `ccn hook` with the event JSON on stdin; this decides
//! what to do and prints a decision when it has one. It never exits non-zero: a failure here
//! must not block Claude.

use anyhow::{Context, Result};
use rust_i18n::t;
use serde_json::Value;
use std::io::Read;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::api::{self, Api, Wait};
use crate::config::{self, Config, Unpaired};
use crate::logger::log;
use crate::payload::{self, Input};
use crate::session::{self, Card, SessionState};
use crate::{live, local, notify, presence, respond};

const EVENT_TIMEOUT: Duration = Duration::from_secs(3);
const QUICK_TIMEOUT: Duration = Duration::from_secs(2);
const SETTINGS_MAX_AGE: i64 = 60 * 60;
const REGISTER_MAX_AGE: i64 = 24 * 60 * 60;

const DAY: i64 = 24 * 60 * 60;

/// How often a SessionStart asks the server whether to nudge
const LICENCE_CHECK_EVERY: i64 = DAY;
/// Once shown, the licence nudge waits this long before it may repeat
const LICENCE_NUDGE_REPEAT: i64 = 7 * DAY;

/// When the licence nudge may show: on the second and sixth day after pairing, and otherwise once
/// a week. Never twice in a day. `paired_at` None (an install from before it was recorded) counts
/// as long ago.
pub fn licence_nudge_allowed(now: i64, paired_at: Option<i64>, last_shown: Option<i64>) -> bool {
    if last_shown.is_some_and(|last| now - last < DAY) {
        return false;
    }
    let days = (now - paired_at.unwrap_or(0)) / DAY;
    let early = days == 2 || days == 6;
    let weekly = match last_shown {
        Some(last) => now - last >= LICENCE_NUDGE_REPEAT,
        None => days >= 7,
    };
    early || weekly
}

/// Shown once, when a SessionStart finds the first phone paired (the user paired from the phone
/// after leaving `ccn pair`, or scanned the QR code later)
fn paired_system_message(phone: &str) -> String {
    t!("hook.paired_message", phone = phone).into_owned()
}

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

    let mut cfg = Config::load();
    let mut state = SessionState::load(&session_id);
    state.touch(&input);
    // Once a day, start a detached look for a newer ccn; the hook itself never waits on it
    let update_message = if event == "SessionStart" {
        crate::update::daily_check(&mut cfg)
    } else {
        None
    };
    if !cfg.is_paired() {
        // No credentials: desktop notifications only, and no network at all
        return Ok(local::run(&mut cfg, &event, &input, &mut state).or(update_message));
    }
    let api = Api::new(&cfg);

    if let Some(mut flag) = Unpaired::load() {
        // No network while unpaired, except a once-a-day check at SessionStart in case the
        // rejection was a server mistake. `ccn pair` deletes the flag.
        let recheck =
            event == "SessionStart" && config::now() - flag.checked_at > Unpaired::RECHECK_AFTER;
        if !(recheck && still_accepted(&api, &mut flag)) {
            return Ok(unpaired(&cfg, &event, &mut state, flag).or(update_message));
        }
        log("the server accepts this computer again");
    }

    // Registered but no phone known to be paired: one quick question at SessionStart, so a phone
    // that paired while no `ccn pair` was watching is picked up
    let announce = if event == "SessionStart" && cfg.phones.unwrap_or(0) == 0 {
        check_phones(&mut cfg, &api)
    } else {
        None
    };
    if cfg.local_mode() {
        let message = local::run(&mut cfg, &event, &input, &mut state);
        return Ok(announce.or(message).or(update_message));
    }

    let mut ctx = Ctx {
        cfg,
        api,
        input,
        state,
    };
    let result = ctx.dispatch(&event);
    if ctx.api.was_rejected() {
        log(&format!(
            "{event}: the server rejected this computer's credentials; run `ccn pair`"
        ));
        return Ok(unpaired(&ctx.cfg, &event, &mut ctx.state, Unpaired::new()).or(update_message));
    }
    match announce {
        Some(message) => Ok(Some(message)),
        None => result.map(|message| message.or(update_message)),
    }
}

/// Asks the server how many phones are paired and records the answer. Returns the one-time
/// "paired with" message when the first phone has arrived since the last look.
fn check_phones(cfg: &mut Config, api: &Api) -> Option<Value> {
    let devices = match api.pairings_within(QUICK_TIMEOUT) {
        Ok(devices) => devices,
        Err(err) => {
            log(&format!("phone check skipped: {err:#}"));
            return None;
        }
    };
    cfg.record_phones(devices.len());
    let mut message = None;
    if !devices.is_empty() && !cfg.paired_message_shown {
        cfg.paired_message_shown = true;
        let newest = devices.iter().max_by_key(|d| d.created_at)?;
        let name = newest
            .name
            .clone()
            .unwrap_or_else(|| t!("common.your_phone").into_owned());
        message = Some(serde_json::json!({ "systemMessage": paired_system_message(&name) }));
    }
    let _ = cfg.save();
    message
}

/// Asks the server whether it accepts this computer after all. Clears the flag when it does.
fn still_accepted(api: &Api, flag: &mut Unpaired) -> bool {
    match api.get_settings(None, QUICK_TIMEOUT) {
        Ok(_) => {
            Unpaired::clear();
            true
        }
        Err(err) => {
            if !api::is_rejected(&err) {
                log(&format!("unpaired re-check failed: {err:#}"));
            }
            flag.checked_at = config::now();
            false
        }
    }
}

/// What a hook does while the server rejects this computer: a desktop notification at most once a
/// day, and a message in Claude Code at SessionStart and on the first prompt of each session.
fn unpaired(
    cfg: &Config,
    event: &str,
    state: &mut SessionState,
    mut flag: Unpaired,
) -> Option<Value> {
    let now = config::now();
    if cfg.desktop_notifications && now - flag.notified_at > Unpaired::NOTIFY_EVERY {
        notify::show(
            notify::APP_NAME,
            &t!("hook.unpaired_notification"),
            cfg.settings.sound(),
        );
        flag.notified_at = now;
    }
    flag.save();
    match event {
        "SessionEnd" => {
            SessionState::remove(&state.session_id);
            None
        }
        "SessionStart" | "UserPromptSubmit" => {
            if event == "UserPromptSubmit" && state.unpaired_warned {
                return None;
            }
            state.unpaired_warned = true;
            let _ = state.save();
            Some(serde_json::json!({ "systemMessage": t!("hook.unpaired_message") }))
        }
        _ => None,
    }
}

impl Ctx {
    fn dispatch(&mut self, event: &str) -> Result<Option<Value>> {
        match event {
            "SessionStart" => self.session_start(),
            "UserPromptSubmit" => self.user_prompt(),
            "PreToolUse" => self.pre_tool_use(),
            "PermissionRequest" => self.permission_request(),
            "Notification" => self.notification(),
            "Stop" => self.stop(),
            "SessionEnd" => self.session_end(),
            other => {
                log(&format!("ignoring event {other}"));
                Ok(None)
            }
        }
    }

    fn input(&self) -> Input<'_> {
        Input(&self.input)
    }

    fn quiet(&self) -> bool {
        self.cfg
            .settings
            .in_quiet_hours(chrono::Local::now().time())
    }

    /// The user is at the terminal Claude runs in, and asked for nothing to be sent then
    fn quiet_at_terminal(&self) -> bool {
        self.cfg.settings.quiet_when_present()
            && presence::at_terminal(self.cfg.settings.presence_threshold())
    }

    // ---- events ----

    fn session_start(&mut self) -> Result<Option<Value>> {
        self.state.started_at = config::now();
        self.state.source = self.input().str("source").map(|s| s.to_string());
        self.state.save()?;
        session::prune_old(7);

        let now = config::now();
        // Registration is an unauthenticated upsert, so it would bring a deleted computer back
        // with no phones. Only re-register once an authenticated call has just been accepted.
        let register_due = now - self.cfg.registered_at.unwrap_or(0) > REGISTER_MAX_AGE;
        let mut accepted = false;
        if register_due || now - self.cfg.settings_fetched_at.unwrap_or(0) > SETTINGS_MAX_AGE {
            match self
                .api
                .get_settings(self.cfg.settings_etag.as_deref(), QUICK_TIMEOUT)
            {
                Ok(Some(result)) => {
                    accepted = true;
                    self.cfg.settings = result.settings;
                    self.cfg.settings_updated_at = result.updated_at;
                    self.cfg.settings_etag = result.etag;
                    self.cfg.settings_fetched_at = Some(now);
                    let _ = self.cfg.save();
                }
                Ok(None) => {
                    accepted = true;
                    self.cfg.settings_fetched_at = Some(now);
                    let _ = self.cfg.save();
                }
                Err(err) if api::is_rejected(&err) => return Err(err),
                Err(err) => log(&format!("settings sync skipped: {err:#}")),
            }
        }
        if register_due && accepted {
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
        Ok(self.licence_nudge(now))
    }

    /// Once a day, asks whether this computer should be told about licences, and tells it when the
    /// cadence allows (`licence_nudge_allowed`). The server writes the message and picks the
    /// variant; `CCN_NO_NUDGE=1` turns it off.
    fn licence_nudge(&mut self, now: i64) -> Option<Value> {
        if local::no_nudge_env()
            || now - self.cfg.licence_checked_at.unwrap_or(0) < LICENCE_CHECK_EVERY
        {
            return None;
        }
        self.cfg.licence_checked_at = Some(now);
        let _ = self.cfg.save();
        let status = match self.api.licence_status(QUICK_TIMEOUT) {
            Ok(status) => status,
            Err(err) => {
                log(&format!("licence check skipped: {err:#}"));
                return None;
            }
        };
        if !status.nudge {
            return None;
        }
        let since = self.cfg.paired_at.or(self.cfg.registered_at);
        if !licence_nudge_allowed(now, since, self.cfg.licence_nudge_last_at) {
            return None;
        }
        self.cfg.licence_nudge_last_at = Some(now);
        let _ = self.cfg.save();
        let Some(message) = status.nudge_message else {
            // A server that says `nudge: true` without writing the message itself (one from
            // before Oct 2026). The line describes this computer, never anyone's store purchase.
            return Some(serde_json::json!({ "systemMessage": t!("hook.licence_message") }));
        };
        if self.cfg.desktop_notifications {
            notify::show(&message.title, &message.body, false);
        }
        crate::telemetry::event(
            "cli_nudge_shown",
            &[
                ("id", &message.id),
                ("kind", &message.kind),
                ("variant", &message.variant),
            ],
        );
        Some(serde_json::json!({
            "systemMessage": t!(
                "hook.nudge_message",
                title = message.title,
                body = message.body,
                command = message.command
            )
        }))
    }

    fn user_prompt(&mut self) -> Result<Option<Value>> {
        self.settle_last_card();
        let now = config::now();
        if let Some(prompt) = self.input().str("prompt") {
            self.state.prompt = Some(SessionState::truncate(prompt, 4000));
            self.state.prompt_at = Some(now);
        }
        self.state.turn = live::Turn::begin(now);
        self.send_working(now, None);
        self.state.save()?;
        Ok(None)
    }

    fn pre_tool_use(&mut self) -> Result<Option<Value>> {
        self.settle_last_card();
        let tool_name = self.input().tool_name();
        self.state.tool_name = tool_name.clone();
        self.state.tool_input = Some(self.input().tool_input());
        self.state.tool_at = Some(config::now());
        if let Some(name) = &tool_name {
            self.state
                .turn
                .record_tool(name, &self.input().tool_input());
        }
        if tool_name.as_deref() != Some("AskUserQuestion") {
            self.send_working(config::now(), tool_name.as_deref());
        }
        self.state.save()?;

        if tool_name.as_deref() == Some("AskUserQuestion") && self.cfg.settings.notify_on_question()
        {
            let tool_input = self.input().tool_input();
            let questions = payload::questions_from_input(&tool_input);
            if questions.is_empty() {
                return Ok(None);
            }
            let (card, key) = payload::question_card(&self.input(), &questions, false);
            let ceiling = self.cfg.settings.hold_ceiling();
            let answer = self.blocking("question", &key, card, ceiling, true)?;
            return Ok(
                answer.and_then(|text| respond::question_output(&text, &tool_input, &questions))
            );
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
        if answer.is_some() {
            self.state.turn.record_approval();
            let _ = self.state.save();
        }
        Ok(answer.map(|text| respond::permission_output(&text)))
    }

    fn notification(&mut self) -> Result<Option<Value>> {
        let kind = self
            .input()
            .str("notification_type")
            .unwrap_or("")
            .to_string();
        if kind != "permission_prompt"
            || !self.cfg.settings.notify_on_permission()
            || self.quiet()
            || self.quiet_at_terminal()
        {
            return Ok(None);
        }
        // A card for this request may already be on the phone from the PermissionRequest hook
        if let Some(card) = &self.state.last_card {
            let fresh = config::now() - card.sent_at < self.cfg.settings.hold_ceiling() as i64 + 60;
            if fresh
                && !card.settled
                && (card.kind == "permission" || card.kind == "plan" || card.kind == "question")
            {
                return Ok(None);
            }
        }
        let (card, kind, key) =
            if self.state.tool_name.is_some() && self.input().tool_name().is_none() {
                // The Notification event carries no tool; use what PreToolUse recorded
                let mut synthetic = self.input.clone();
                synthetic["tool_name"] =
                    Value::String(self.state.tool_name.clone().unwrap_or_default());
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
        if !stop_hook_active && self.live_updates_allowed() {
            // Before the done card, which may hold for the phone's answer
            let update = live::done(
                &self.state.session_id,
                self.input().project_name().as_deref(),
                self.input().str("last_assistant_message"),
                &self.state.turn,
                config::now(),
            );
            if let Err(err) = self.api.post_event(&update, QUICK_TIMEOUT) {
                log(&format!("could not send the done update: {err:#}"));
            }
        }
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
        let _ = self.api.post_event(
            &payload::session_update(&session_id, "ended"),
            QUICK_TIMEOUT,
        );
        SessionState::remove(&session_id);
        Ok(None)
    }

    // ---- helpers ----

    /// The phone's lock-screen card follows the session unless the user asked for quiet
    fn live_updates_allowed(&self) -> bool {
        self.cfg.settings.notify_on_done() && !self.quiet() && !self.quiet_at_terminal()
    }

    /// A throttled "Claude is working" update for the lock-screen card. Never fails the hook.
    fn send_working(&mut self, now: i64, tool_name: Option<&str>) {
        if !self.live_updates_allowed() || !self.state.turn.working_due(now) {
            return;
        }
        let tool_input = self.input().tool_input();
        let update = live::working(
            &self.state.session_id,
            self.input().project_name().as_deref(),
            self.state.prompt.as_deref(),
            tool_name.map(|name| (name, &tool_input)),
            self.state.turn.progress,
            self.state.turn.started_at,
        );
        if let Err(err) = self.api.post_event(&update, QUICK_TIMEOUT) {
            log(&format!("could not send the working update: {err:#}"));
        }
    }

    /// Tells the server the previous request in this session was answered at the keyboard.
    fn settle_last_card(&mut self) {
        let Some(card) = &mut self.state.last_card else {
            return;
        };
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

    fn send_card(
        &mut self,
        kind: &str,
        key: &str,
        mut card: Value,
        held: bool,
    ) -> Result<Option<String>> {
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
            let attention = t!("hook.needs_attention");
            let body = card["promptText"]
                .as_str()
                .or(card["message"].as_str())
                .unwrap_or(&attention);
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
            // At the keyboard: nothing waits. The phone gets a note, unless the user is looking at
            // this very terminal and has asked for quiet then
            if present_sends_info && !self.quiet_at_terminal() {
                self.send_card(kind, key, card, false)?;
            }
            return Ok(None);
        }

        let Some(id) = self.send_card(kind, key, card, true)? else {
            return Ok(None);
        };
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
                    Err(err) if api::is_rejected(&err) => {
                        let _ = tx.send(None);
                        return;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licence_nudges_on_day_two_and_six_then_weekly() {
        let paired = 1_700_000_000;
        let at = |day: i64| paired + day * DAY + 3600;
        // Days 0 and 1: nothing
        assert!(!licence_nudge_allowed(at(0), Some(paired), None));
        assert!(!licence_nudge_allowed(at(1), Some(paired), None));
        // Day 2 yes, day 3 no (even though never shown), day 6 yes
        assert!(licence_nudge_allowed(at(2), Some(paired), None));
        assert!(!licence_nudge_allowed(at(3), Some(paired), Some(at(2))));
        assert!(!licence_nudge_allowed(at(3), Some(paired), None));
        assert!(licence_nudge_allowed(at(6), Some(paired), Some(at(2))));
        // Weekly after that: day 7 is too soon after day 6, day 13 is a week on
        assert!(!licence_nudge_allowed(at(7), Some(paired), Some(at(6))));
        assert!(licence_nudge_allowed(at(13), Some(paired), Some(at(6))));
        assert!(!licence_nudge_allowed(at(19), Some(paired), Some(at(13))));
        assert!(licence_nudge_allowed(at(20), Some(paired), Some(at(13))));
        // Never shown and the early days missed: from day 7 on
        assert!(licence_nudge_allowed(at(9), Some(paired), None));
        // Never twice in a day, even on a nudge day
        assert!(!licence_nudge_allowed(
            at(2) + 60,
            Some(paired),
            Some(at(2))
        ));
        // No pairing date on record counts as long ago
        assert!(licence_nudge_allowed(at(0), None, None));
        assert!(!licence_nudge_allowed(at(0), None, Some(at(0) - 3 * DAY)));
    }
}
