//! Licences bought on the web: `ccn unlock`, `ccn activate`, `ccn deactivate`, `ccn licence`.
//!
//! A licence covers up to 3 computers, and every phone paired with a licensed computer is unlocked.
//! Keys are only ever typed here or on the website's licence page, never in the phone apps.

use anyhow::{Result, bail};
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use crate::api::{self, Api, CheckoutWait, Licence, LicenceComputer, LicenceStatus};
use crate::commands::ensure_registered;
use crate::config::{self, Config};
use crate::{qr, telemetry};

/// How long `ccn unlock` waits for the payment before leaving it to the emailed key.
const CHECKOUT_WAIT: Duration = Duration::from_secs(15 * 60);
/// A computer with this machine's name not seen for this long is probably this one, reinstalled.
const STALE_SLOT_SECS: i64 = 7 * 24 * 3600;

pub fn unlock(lifetime: bool) -> Result<()> {
    let plan = if lifetime { "lifetime" } else { "annual" };
    let mut cfg = Config::load();
    let api = ensure_registered(&mut cfg)?;
    // Both names, so the dashboards built on the old one keep working
    telemetry::event("cli_upgrade_started", &[("plan", plan)]);
    telemetry::event("cli_unlock_started", &[("plan", plan)]);

    // An annual licence asking for lifetime is an upgrade of the same licence
    let current = api.licence_status(Duration::from_secs(10)).ok();
    let upgrade = lifetime
        && current
            .as_ref()
            .and_then(|s| s.licence.as_ref())
            .map(|l| l.plan == "annual" && s_licensed(&current))
            .unwrap_or(false);

    let checkout = match api.start_checkout(plan, upgrade) {
        Ok(c) => c,
        Err(err) => match api::licence_error(&err) {
            Some(e) if e.code == "ALREADY_LICENSED" => {
                let licence = e.licence();
                println!(
                    "This computer is already licensed ({}).",
                    licence.as_ref().map(describe).unwrap_or_default()
                );
                if licence.map(|l| l.plan == "annual").unwrap_or(false) {
                    println!(
                        "To switch to a lifetime licence, keeping the same key: ccn unlock --lifetime"
                    );
                }
                return Ok(());
            }
            _ => return Err(err),
        },
    };

    let opened = open_browser(&checkout.url);
    telemetry::event(
        "cli_checkout_opened",
        &[("opened_browser", if opened { "true" } else { "false" })],
    );
    println!();
    if opened {
        println!(
            "  The checkout is open in your browser. To pay on your phone instead, scan this:"
        );
    } else {
        println!("  Open this link to pay, or scan it with your phone:");
    }
    println!();
    if let Some(code) = qr::render(&checkout.url) {
        print!("{code}");
        println!();
    }
    println!("  {}", checkout.url);
    println!();
    println!("  Waiting for the payment... (Ctrl+C to stop; the key is emailed to you either way)");

    let deadline = Instant::now() + CHECKOUT_WAIT;
    while Instant::now() < deadline {
        match api.wait_checkout(&checkout.session_id, 25) {
            Ok(CheckoutWait::Pending) => {}
            Ok(CheckoutWait::Complete {
                key,
                activated,
                upgraded,
                licence,
            }) => {
                telemetry::event("cli_licensed", &[("method", "checkout")]);
                println!();
                if upgraded {
                    println!(
                        "✓ Upgraded to a lifetime licence. Same key, same computers; the annual renewal is cancelled."
                    );
                    return Ok(());
                }
                println!("✓ This computer is licensed ({}).", describe(&licence));
                if let Some(key) = key {
                    println!(
                        "  Key: {key} (also emailed). Use it on up to 2 more computers: ccn activate {key}"
                    );
                }
                if activated {
                    println!("  Every phone paired with this computer is unlocked.");
                } else {
                    println!(
                        "  Run `ccn activate <key>` with the emailed key to use it on this computer."
                    );
                }
                return Ok(());
            }
            Err(err) if api::is_rejected(&err) => return Err(err),
            Err(_) => std::thread::sleep(Duration::from_secs(5)),
        }
    }
    println!();
    println!(
        "Still no payment after 15 minutes. If you paid, the key is in your email: ccn activate <key>"
    );
    Ok(())
}

fn s_licensed(status: &Option<LicenceStatus>) -> bool {
    status.as_ref().map(|s| s.licensed).unwrap_or(false)
}

pub fn activate(key: &str, replace: Option<String>) -> Result<()> {
    let mut cfg = Config::load();
    let api = ensure_registered(&mut cfg)?;
    let result = match api.activate_licence(key, replace.as_deref()) {
        Err(err) => match api::licence_error(&err) {
            Some(e) if e.code == "SLOTS_FULL" && replace.is_none() => {
                let computers = e.computers();
                match reinstalled_slot(&computers, &cfg.machine_name()) {
                    Some(old)
                        if confirm(&format!(
                            "\"{}\" hasn't been seen for over a week. Is that this computer? Replace it?",
                            name_of(old)
                        )) =>
                    {
                        api.activate_licence(key, Some(&old.id))
                    }
                    _ => {
                        telemetry::event("cli_activate_failed", &[("reason", "slots_full")]);
                        println!("This licence is already on {} computers:", computers.len());
                        print_computers(&computers);
                        println!();
                        println!(
                            "Free one with `ccn deactivate` on that computer, or replace it from here:"
                        );
                        println!(
                            "  ccn activate {key} --replace \"{}\"",
                            computers.first().map(name_of).unwrap_or("Old laptop")
                        );
                        bail!("no free computer slot");
                    }
                }
            }
            Some(e) => {
                let reason = e.code.to_lowercase();
                telemetry::event("cli_activate_failed", &[("reason", reason.as_str())]);
                if e.code == "NO_SUCH_COMPUTER" {
                    println!("Computers on this licence:");
                    print_computers(&e.computers());
                }
                return Err(err);
            }
            None => {
                telemetry::event(
                    "cli_activate_failed",
                    &[("reason", telemetry::failure_reason(&err))],
                );
                return Err(err);
            }
        },
        ok => ok,
    }?;

    telemetry::event("cli_licensed", &[("method", "key")]);
    let licence = result.licence.as_ref().map(describe).unwrap_or_default();
    println!(
        "✓ Licensed ({licence}). {} of 3 computers in use.",
        result.computers.len()
    );
    if !cfg.is_paired()
        || Api::new(&cfg)
            .pairings()
            .map(|d| d.is_empty())
            .unwrap_or(false)
    {
        println!("  Pair a phone with `ccn pair` and it is unlocked straight away.");
    } else {
        println!("  Every phone paired with this computer is unlocked.");
    }
    Ok(())
}

pub fn deactivate() -> Result<()> {
    let cfg = Config::load();
    if !cfg.is_paired() {
        println!("This computer has no licence.");
        return Ok(());
    }
    let api = Api::new(&cfg);
    let before = api.licence_status(Duration::from_secs(10))?;
    if !api.deactivate_licence()? {
        println!("This computer has no licence.");
        return Ok(());
    }
    telemetry::event("cli_deactivated", &[]);
    let left = before.computers.len().saturating_sub(1);
    println!("Freed. This licence now has {left} of 3 computers in use.");
    Ok(())
}

pub fn show() -> Result<()> {
    let cfg = Config::load();
    if !cfg.is_paired() {
        println!("No licence on this computer. Buy one: ccn unlock");
        return Ok(());
    }
    let status = Api::new(&cfg).licence_status(Duration::from_secs(10))?;
    let Some(licence) = &status.licence else {
        println!("No licence on this computer.");
        println!("  Buy one:            ccn unlock (annual) or ccn unlock --lifetime");
        println!("  Already have a key: ccn activate <key>");
        return Ok(());
    };
    println!("Licence:   {}", describe(licence));
    println!("Key:       CCN-…-{}", licence.key_last4);
    println!(
        "Unlocks:   {}",
        if status.licensed {
            "yes, every phone paired with this computer"
        } else {
            "no"
        }
    );
    println!("Computers: {} of 3", status.computers.len());
    print_computers(&status.computers);
    if licence.plan == "annual" && status.licensed {
        println!();
        println!("Switch to lifetime, keeping the key: ccn unlock --lifetime");
    }
    Ok(())
}

/// One line for bare `ccn` and `ccn doctor`, or None when the server can't be asked quickly.
pub fn status_line(api: &Api) -> Option<String> {
    let status = api.licence_status(Duration::from_secs(5)).ok()?;
    Some(match &status.licence {
        Some(l) if status.licensed => {
            format!("{}, {} of 3 computers", describe(l), status.computers.len())
        }
        Some(l) => format!("{}: ccn unlock", describe(l)),
        None => "none: ccn unlock".to_string(),
    })
}

/// "annual, renews 27 Oct 2027", "lifetime", "annual, ended 2 Jan 2027", "refunded"
pub fn describe(l: &Licence) -> String {
    let date = l
        .valid_until
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| t.format("%-d %b %Y").to_string());
    match (l.plan.as_str(), l.status.as_str(), date) {
        (_, "refunded", _) => "refunded".to_string(),
        ("lifetime", _, _) => "lifetime".to_string(),
        (_, "active", Some(d)) => format!("annual, renews {d}"),
        (_, "past_due", _) => "annual, payment retrying".to_string(),
        (_, "cancelled", Some(d)) => format!("annual, ends {d}"),
        (_, "expired", Some(d)) => format!("annual, ended {d}"),
        (plan, status, _) => format!("{plan}, {status}"),
    }
}

fn name_of(c: &LicenceComputer) -> &str {
    c.name.as_deref().unwrap_or("unnamed computer")
}

fn print_computers(computers: &[LicenceComputer]) {
    for c in computers {
        let seen = c
            .last_seen_at
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|t| format!("last seen {}", t.format("%-d %b %Y")))
            .unwrap_or_else(|| "not seen recently".to_string());
        println!(
            "  {:<28} {}{}",
            name_of(c),
            seen,
            if c.current { "  (this computer)" } else { "" }
        );
    }
}

/// A slot held by a computer with this machine's name that hasn't been seen for a week: most likely
/// this computer before a reinstall gave it a new identity.
fn reinstalled_slot<'a>(
    computers: &'a [LicenceComputer],
    machine_name: &str,
) -> Option<&'a LicenceComputer> {
    let now = config::now();
    computers.iter().find(|c| {
        c.name
            .as_deref()
            .map(|n| n.eq_ignore_ascii_case(machine_name))
            .unwrap_or(false)
            && now - c.last_seen_at.unwrap_or(0) > STALE_SLOT_SECS
    })
}

fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    !answer.trim().to_lowercase().starts_with('n')
}

/// Opens the checkout in the default browser. Over SSH there is no browser to open here.
fn open_browser(url: &str) -> bool {
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        return false;
    }
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        // Not `cmd /c start`: cmd would split the URL at its `&`s
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn licence(plan: &str, status: &str, valid_until: Option<i64>) -> Licence {
        Licence {
            plan: plan.into(),
            status: status.into(),
            valid_until,
            key_last4: "ABCD".into(),
        }
    }

    #[test]
    fn describes_each_state() {
        let oct_27_2027 = 1824674083; // 2027-10-27T21:54:43Z
        assert_eq!(
            describe(&licence("annual", "active", Some(oct_27_2027))),
            "annual, renews 27 Oct 2027"
        );
        assert_eq!(
            describe(&licence("annual", "cancelled", Some(oct_27_2027))),
            "annual, ends 27 Oct 2027"
        );
        assert_eq!(
            describe(&licence("annual", "expired", Some(oct_27_2027))),
            "annual, ended 27 Oct 2027"
        );
        assert_eq!(
            describe(&licence("annual", "past_due", None)),
            "annual, payment retrying"
        );
        assert_eq!(describe(&licence("lifetime", "active", None)), "lifetime");
        assert_eq!(describe(&licence("lifetime", "refunded", None)), "refunded");
    }

    #[test]
    fn a_stale_slot_with_this_name_is_offered_for_replacement() {
        let now = config::now();
        let computer = |name: &str, seen: i64| LicenceComputer {
            id: format!("id-{name}"),
            name: Some(name.into()),
            platform: None,
            activated_at: 0,
            last_seen_at: Some(seen),
            current: false,
        };
        let computers = vec![
            computer("Desk", now),
            computer("MacBook", now - 8 * 24 * 3600),
            computer("Build box", now - 30 * 24 * 3600),
        ];
        assert_eq!(
            reinstalled_slot(&computers, "macbook").map(|c| c.id.as_str()),
            Some("id-MacBook")
        );
        assert!(
            reinstalled_slot(&computers, "Desk").is_none(),
            "seen today, so not a reinstall"
        );
        assert!(reinstalled_slot(&computers, "Laptop").is_none());
    }
}
