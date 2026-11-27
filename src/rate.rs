//! Asks for a store rating once per install, after the phone has answered `RATE_AFTER_ANSWERS`
//! held requests: a desktop notification that opens the server's /rate page (which sends the
//! phone to its store), plus a line in Claude Code. Only at a SessionStart with nothing else to
//! say, and never within a day of the licence nudge or the suggestion of a sister app.
//! `ccn config set rate_prompt off`, or `CCN_NO_NUDGE=1`, keeps it quiet.

use rust_i18n::t;
use serde_json::{Value, json};

use crate::config::Config;
use crate::{local, notify};

/// Answers from the phone before the prompt
pub const RATE_AFTER_ANSWERS: u64 = 20;
const DAY: i64 = 24 * 60 * 60;

pub fn enabled(cfg: &Config) -> bool {
    cfg.rate_prompt && !local::no_nudge_env()
}

/// One more request answered from the phone
pub fn record_answer(cfg: &mut Config) {
    cfg.phone_answers += 1;
    let _ = cfg.save();
}

/// Remembers which platforms the paired phones run.
pub fn record_platforms<'a>(cfg: &mut Config, platforms: impl IntoIterator<Item = &'a str>) {
    let seen = normalise_platforms(platforms);
    if seen != cfg.phone_platforms {
        cfg.phone_platforms = seen;
        let _ = cfg.save();
    }
}

/// ios and android, lowercase, sorted, once each; anything else is ignored.
fn normalise_platforms<'a>(platforms: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen: Vec<String> = platforms
        .into_iter()
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| p == "ios" || p == "android")
        .collect();
    seen.sort();
    seen.dedup();
    seen
}

/// The phones' one platform, or None when there are both (or none known): the page then asks.
fn platform(platforms: &[String]) -> Option<&str> {
    match platforms {
        [only] => Some(only.as_str()),
        _ => None,
    }
}

/// The server's rating page for this computer's phones
pub fn url(server_url: &str, platforms: &[String]) -> String {
    let base = server_url.trim_end_matches('/');
    match platform(platforms) {
        Some(p) => format!("{base}/rate?app=ccn&platform={p}&ref=cli"),
        None => format!("{base}/rate?app=ccn&ref=cli"),
    }
}

pub fn due(now: i64, answers: u64, shown_at: Option<i64>, other_nudges: &[Option<i64>]) -> bool {
    shown_at.is_none()
        && answers >= RATE_AFTER_ANSWERS
        && !other_nudges.iter().flatten().any(|last| now - last < DAY)
}

/// The SessionStart `systemMessage` and desktop notification, when due (paired mode only). Call it
/// only when nothing else is being shown this SessionStart.
pub fn session_message(cfg: &mut Config, now: i64) -> Option<Value> {
    if !enabled(cfg)
        || !due(
            now,
            cfg.phone_answers,
            cfg.rate_prompt_shown_at,
            &[cfg.licence_nudge_last_at, cfg.cross_promo_last_at],
        )
    {
        return None;
    }
    cfg.rate_prompt_shown_at = Some(now);
    let _ = cfg.save();
    let link = url(&cfg.server_url, &cfg.phone_platforms);
    if cfg.desktop_notifications && notify::available() {
        notify::show_link(notify::APP_NAME, &t!("rate.notification_body"), &link);
    }
    let platform = platform(&cfg.phone_platforms).unwrap_or("choose");
    crate::telemetry::event_detached("cli_rate_prompt_shown", &[("platform", platform)]);
    Some(json!({ "systemMessage": t!("rate.session_message", url = link) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn once_after_twenty_answers_and_never_beside_another_nudge() {
        let now = 1_000_000_000;
        assert!(!due(now, 19, None, &[]));
        assert!(due(now, 20, None, &[]));
        assert!(!due(now, 50, Some(now - 400 * DAY), &[]));
        assert!(!due(now, 20, None, &[None, Some(now - 60)]));
        assert!(due(now, 20, None, &[Some(now - DAY), None]));
    }

    #[test]
    fn the_url_names_the_platform_only_when_there_is_one() {
        let api = "https://api.claudecodenotifier.com/";
        assert_eq!(
            url(api, &["ios".to_string()]),
            "https://api.claudecodenotifier.com/rate?app=ccn&platform=ios&ref=cli"
        );
        assert_eq!(
            url(api, &["android".to_string(), "ios".to_string()]),
            "https://api.claudecodenotifier.com/rate?app=ccn&ref=cli"
        );
        assert_eq!(
            url(api, &[]),
            "https://api.claudecodenotifier.com/rate?app=ccn&ref=cli"
        );
    }

    #[test]
    fn platforms_are_deduplicated_and_filtered() {
        assert_eq!(
            normalise_platforms(["ios", "iOS", "android", "web"]),
            vec!["android", "ios"]
        );
        assert!(normalise_platforms([]).is_empty());
    }
}
