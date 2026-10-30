//! Local mode: no phone is paired with this computer, so the hooks show desktop notifications,
//! never touch the network and never hold Claude. A counter and a few nudges suggest pairing.
//!
//! The text here is what the user sees; keep each piece whole so it can be translated later.

use serde_json::{Value, json};

use crate::config::{self, Config};
use crate::payload::{self, Input};
use crate::session::{self, SessionState};
use crate::{notify, presence};

const DAY: i64 = 24 * 60 * 60;
/// How often the pair message repeats at SessionStart once the 10th notification has asked for it
const REPEAT_EVERY: i64 = 7 * DAY;
/// The local notification whose arrival earns a nudge of its own
pub const MILESTONE: u64 = 10;

pub const FINISHED_TITLE: &str = "Claude has finished";
pub const FINISHED_BODY: &str = "Claude has finished and is waiting for you.";
pub const PERMISSION_TITLE: &str = "Claude needs permission";
pub const QUESTION_TITLE: &str = "Claude has a question";

/// Appended to the body of the very first local notification
pub const FIRST_NUDGE_LINE: &str = "Get these on your phone: ccn pair";
/// A notification of its own, after the 10th
pub const MILESTONE_NUDGE: &str = "10 notifications so far. Get them on your phone, with approvals from the lock screen: run ccn pair";
/// Shown in Claude Code after the 10th, then every week at SessionStart
pub const PAIR_SYSTEM_MESSAGE: &str = "Claude Code Notifier: you get desktop notifications. To get them on your phone too, and approve or answer from there, run `ccn pair` in a terminal.";

/// `ccn config set pair_nudges off`, or `CCN_NO_NUDGE=1`, keeps every suggestion to pair quiet.
pub fn nudges_enabled(cfg: &Config) -> bool {
    cfg.pair_nudges && !no_nudge_env()
}

pub fn no_nudge_env() -> bool {
    std::env::var("CCN_NO_NUDGE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// What the `count`th local notification (1-based) adds on top of itself.
#[derive(Debug, PartialEq)]
pub enum Milestone {
    None,
    /// Append `FIRST_NUDGE_LINE` to the body
    First,
    /// Show `MILESTONE_NUDGE` as well, and ask Claude Code to show `PAIR_SYSTEM_MESSAGE`
    Tenth,
}

pub fn milestone(count: u64) -> Milestone {
    match count {
        1 => Milestone::First,
        MILESTONE => Milestone::Tenth,
        _ => Milestone::None,
    }
}

/// Whether `PAIR_SYSTEM_MESSAGE` is due. The 10th notification sets `due`, which the next
/// SessionStart or prompt consumes; after that only SessionStart repeats it, every week. Never
/// more than once a day either way.
pub fn pair_message_due(due: bool, last_at: Option<i64>, now: i64, at_session_start: bool) -> bool {
    let not_today = last_at.is_none_or(|last| now - last >= DAY);
    if due {
        return not_today;
    }
    at_session_start && last_at.is_some_and(|last| now - last >= REPEAT_EVERY)
}

/// The nudge's body with the message shortened so the whole thing fits a notification.
pub fn body_with_nudge(body: &str) -> String {
    format!(
        "{}\n{FIRST_NUDGE_LINE}",
        // notify::show caps the body at 200 characters, and truncate adds "..."
        SessionState::truncate(body, 200 - FIRST_NUDGE_LINE.len() - 4)
    )
}

/// The hook's work while no phone is paired. Returns a `systemMessage` for Claude Code or None.
pub fn run(
    cfg: &mut Config,
    event: &str,
    input: &Value,
    state: &mut SessionState,
) -> Option<Value> {
    let now = config::now();
    let inp = Input(input);
    match event {
        "SessionStart" => {
            state.started_at = now;
            state.source = inp.str("source").map(|s| s.to_string());
            let _ = state.save();
            session::prune_old(7);
            pair_message(cfg, now, true)
        }
        "UserPromptSubmit" => {
            if let Some(prompt) = inp.str("prompt") {
                state.prompt = Some(SessionState::truncate(prompt, 4000));
                state.prompt_at = Some(now);
            }
            let _ = state.save();
            pair_message(cfg, now, false)
        }
        "PreToolUse" => {
            let tool_name = inp.tool_name();
            let tool_input = inp.tool_input();
            state.tool_name = tool_name.clone();
            state.tool_input = Some(tool_input.clone());
            state.tool_at = Some(now);
            let _ = state.save();
            if tool_name.as_deref() == Some("AskUserQuestion") && cfg.settings.notify_on_question()
            {
                let first = payload::first_question(&tool_input)
                    .unwrap_or_else(|| QUESTION_TITLE.to_string());
                show(cfg, QUESTION_TITLE, &first);
            }
            None
        }
        "Notification" => {
            if inp.str("notification_type") != Some("permission_prompt")
                || !cfg.settings.notify_on_permission()
            {
                return None;
            }
            // The Notification event carries no tool; use what PreToolUse recorded
            let (card, _, _) = if state.tool_name.is_some() && inp.tool_name().is_none() {
                let mut synthetic = input.clone();
                synthetic["tool_name"] = Value::String(state.tool_name.clone().unwrap_or_default());
                synthetic["tool_input"] = state.tool_input.clone().unwrap_or(Value::Null);
                payload::permission_card(&Input(&synthetic), false)
            } else {
                payload::permission_card(&inp, false)
            };
            let body = card["promptText"]
                .as_str()
                .or(card["message"].as_str())
                .unwrap_or(PERMISSION_TITLE);
            show(cfg, PERMISSION_TITLE, body);
            None
        }
        "Stop" => {
            if !cfg.settings.notify_on_done()
                || input["stop_hook_active"].as_bool().unwrap_or(false)
            {
                return None;
            }
            let project = inp.project_name();
            let body = inp
                .str("last_assistant_message")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(FINISHED_BODY);
            show(cfg, project.as_deref().unwrap_or(FINISHED_TITLE), body);
            None
        }
        "SessionEnd" => {
            SessionState::remove(&state.session_id);
            None
        }
        _ => None,
    }
}

fn pair_message(cfg: &mut Config, now: i64, at_session_start: bool) -> Option<Value> {
    if !nudges_enabled(cfg)
        || !pair_message_due(
            cfg.pair_nudge_due,
            cfg.pair_nudge_last_at,
            now,
            at_session_start,
        )
    {
        return None;
    }
    cfg.pair_nudge_due = false;
    cfg.pair_nudge_last_at = Some(now);
    let _ = cfg.save();
    Some(json!({ "systemMessage": PAIR_SYSTEM_MESSAGE }))
}

/// Shows one desktop notification, when the settings allow, and counts it towards the nudges.
fn show(cfg: &mut Config, title: &str, body: &str) {
    if !cfg.desktop_notifications
        || cfg.settings.in_quiet_hours(chrono::Local::now().time())
        || (cfg.settings.quiet_when_present()
            && presence::at_terminal(cfg.settings.presence_threshold()))
    {
        return;
    }
    let sound = cfg.settings.sound();
    cfg.local_notifications += 1;
    let nudge = if nudges_enabled(cfg) {
        milestone(cfg.local_notifications)
    } else {
        Milestone::None
    };
    match nudge {
        Milestone::First => notify::show(title, &body_with_nudge(body), sound),
        Milestone::None => notify::show(title, body, sound),
        Milestone::Tenth => {
            notify::show(title, body, sound);
            notify::show(notify::APP_NAME, MILESTONE_NUDGE, false);
            cfg.pair_nudge_due = true;
        }
    }
    let _ = cfg.save();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_and_tenth_notifications_carry_a_nudge() {
        assert_eq!(milestone(1), Milestone::First);
        assert_eq!(milestone(2), Milestone::None);
        assert_eq!(milestone(9), Milestone::None);
        assert_eq!(milestone(10), Milestone::Tenth);
        assert_eq!(milestone(11), Milestone::None);
        assert_eq!(milestone(20), Milestone::None);
    }

    #[test]
    fn the_due_flag_is_consumed_at_session_start_or_prompt() {
        let now = 1_700_000_000;
        assert!(pair_message_due(true, None, now, true));
        assert!(pair_message_due(true, None, now, false));
        // but still at most once a day
        assert!(!pair_message_due(true, Some(now - 3600), now, true));
        assert!(pair_message_due(true, Some(now - DAY), now, true));
    }

    #[test]
    fn repeats_weekly_at_session_start_only() {
        let now = 1_700_000_000;
        // Never shown and nothing due: nothing before the 10th notification asks
        assert!(!pair_message_due(false, None, now, true));
        assert!(!pair_message_due(false, Some(now - 6 * DAY), now, true));
        assert!(pair_message_due(false, Some(now - 7 * DAY), now, true));
        assert!(pair_message_due(false, Some(now - 30 * DAY), now, true));
        // A prompt never repeats it
        assert!(!pair_message_due(false, Some(now - 30 * DAY), now, false));
    }

    #[test]
    fn the_nudge_line_fits_with_the_message() {
        let long = "x".repeat(500);
        let body = body_with_nudge(&long);
        assert!(body.ends_with(&format!("\n{FIRST_NUDGE_LINE}")));
        assert!(body.chars().count() <= 200, "{}", body.chars().count());
        assert_eq!(body_with_nudge("done"), format!("done\n{FIRST_NUDGE_LINE}"));
    }
}
