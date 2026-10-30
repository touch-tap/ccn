//! The user-facing commands: pair, setup, doctor, test, devices, config, sync, uninstall, forget.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

use crate::api::{self, Api};
use crate::config::{self, Config, Unpaired};
use crate::{hookcfg, licence, notify, payload, presence, qr, telemetry};

/// What this computer tells the server about itself. None until credentials exist.
pub fn identity(cfg: &Config) -> Option<Value> {
    let id = cfg.client_id.clone()?;
    let token = config::load_token(cfg)?;
    Some(json!({
        "id": id,
        "authToken": token,
        "name": cfg.machine_name(),
        "platform": config::platform(),
        "clientKind": "cli",
        "clientVersion": config::VERSION,
        "capabilities": config::CAPABILITIES,
    }))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// This computer's API client, registering it with the server first when it has no credentials yet.
/// A licence needs a registered computer, not a paired phone, so `ccn unlock` and `ccn activate`
/// work before (or without) `ccn pair`.
pub fn ensure_registered(cfg: &mut Config) -> Result<Api> {
    if cfg.is_paired() {
        return Ok(Api::new(cfg));
    }
    let client_id = cfg.client_id.clone().unwrap_or_else(new_id);
    let token = new_id();
    let machine_name = cfg.machine_name();
    let api = Api::with_token(cfg, token.clone());
    api.register(&json!({
        "id": client_id,
        "authToken": token,
        "name": machine_name,
        "platform": config::platform(),
        "clientKind": "cli",
        "clientVersion": config::VERSION,
        "capabilities": config::CAPABILITIES,
    }))?;
    cfg.client_id = Some(client_id);
    cfg.machine_name = Some(machine_name);
    cfg.registered_at = Some(config::now());
    cfg.phones = Some(0);
    cfg.save()?;
    config::store_token(cfg, &token)?;
    Ok(api)
}

/// What the user sees when the phone's code is refused or malformed. One whole paragraph, so it
/// can be translated as one.
pub const CODE_NOT_RECOGNISED: &str = "That code wasn't recognised. The 6-digit code is in the Claude Code Notifier app under Computers > Add a computer; it changes every 24 hours, so copy the line the app shows right now.";
const OFFER_QR: &str =
    "Press Enter to show a QR code here instead (scan it with your phone), or Ctrl+C.";
const OPTIONS_WITHOUT_TERMINAL: &str = "Run `ccn pair <code>` again with the code the app shows now, or run `ccn pair` to show a QR code here and scan it with your phone.";

/// Why `ccn pair` returned without an error.
pub enum PairOutcome {
    /// A phone paired
    Paired,
    /// `--no-wait`: the code is on screen and the phone finishes the pairing on its own
    CodeShown,
}

pub fn pair(
    code: Option<String>,
    name: Option<String>,
    replace_mac_app: bool,
    no_wait: bool,
) -> Result<()> {
    let mut flow = if code.is_some() {
        "phone_code"
    } else {
        "computer_code"
    };
    let method = telemetry::install_method();
    telemetry::event("cli_pair_started", &[("flow", flow), ("method", method)]);
    let result = pair_inner(code, name, replace_mac_app, no_wait, &mut flow);
    match &result {
        Ok(PairOutcome::Paired) => {
            telemetry::event("cli_paired", &[("flow", flow), ("method", method)])
        }
        Ok(PairOutcome::CodeShown) => {}
        Err(err) => telemetry::event(
            "cli_pair_failed",
            &[
                ("flow", flow),
                ("method", method),
                ("reason", telemetry::failure_reason(err)),
            ],
        ),
    }
    result.map(|_| ())
}

/// The phone that just paired, and the welcome notification the server pushed to it
type PairedPhone = (String, Option<String>);

fn pair_inner(
    code: Option<String>,
    name: Option<String>,
    replace_mac_app: bool,
    no_wait: bool,
    flow: &mut &'static str,
) -> Result<PairOutcome> {
    // Hooks first: a computer that never finishes pairing still gets desktop notifications
    ensure_setup(replace_mac_app)?;
    let mut cfg = Config::load();
    if let Some(n) = name {
        cfg.machine_name = Some(n);
    }
    let machine_name = cfg.machine_name();
    // A computer the server has deleted (60 days unused) cannot come back under its old identity's
    // pairings, so it starts over as a new one
    let mut registered = cfg.is_paired();
    if registered && matches!(Api::new(&cfg).me(), Err(err) if api::is_rejected(&err)) {
        println!("This computer's old pairing has been removed, so it will be registered again.");
        cfg.client_id = None;
        cfg.phones = None;
        cfg.paired_at = None;
        cfg.paired_message_shown = false;
        registered = false;
    }
    let client_id = cfg.client_id.clone().unwrap_or_else(new_id);
    let token = if registered {
        config::load_token(&cfg).unwrap_or_else(new_id)
    } else {
        new_id()
    };
    let api = Api::with_token(&cfg, token.clone());
    let identity = json!({
        "id": client_id,
        "authToken": token,
        "name": machine_name,
        "platform": config::platform(),
        "clientKind": "cli",
        "clientVersion": config::VERSION,
        "capabilities": config::CAPABILITIES,
    });

    let mut paired_with: Option<PairedPhone> = None;
    if let Some(code) = code {
        match claim_with_code(&api, &identity, &code) {
            Ok(phone) => paired_with = Some(phone),
            Err(err) if code_was_refused(&err) => {
                // Explain, and offer the QR code in place of a dead end
                println!("{CODE_NOT_RECOGNISED}");
                if !offer_qr_instead()? {
                    return Err(err);
                }
                let method = telemetry::install_method();
                telemetry::event(
                    "cli_pair_failed",
                    &[
                        ("flow", flow),
                        ("method", method),
                        ("reason", telemetry::failure_reason(&err)),
                    ],
                );
                *flow = "computer_code";
                telemetry::event(
                    "cli_pair_started",
                    &[("flow", flow), ("method", method), ("recovered", "qr")],
                );
            }
            Err(err) => return Err(err),
        }
    }

    if paired_with.is_none() {
        // This computer shows a code (as a QR code and in digits), the phone takes it
        let before: Vec<String> = if registered {
            list_phones(&mut cfg, &api)
                .map(|d| d.into_iter().map(|d| d.id).collect())
                .unwrap_or_default()
        } else {
            vec![]
        };
        if !registered {
            api.register(&identity)?;
            cfg.client_id = Some(client_id.clone());
            cfg.machine_name = Some(machine_name.clone());
            cfg.registered_at = Some(config::now());
            cfg.phones = Some(0);
            cfg.save()?;
            config::store_token(&cfg, &token)?;
        }
        let issued = api.request_code()?;
        println!();
        match issued
            .link_url
            .as_deref()
            .and_then(|url| qr::render(url).map(|code| (url, code)))
        {
            Some((url, code)) => {
                println!(
                    "  Scan this with your phone's camera to connect it. Without the app yet, it"
                );
                println!("  takes you to the App Store or Google Play: install, then scan again.");
                println!();
                print!("{code}");
                println!();
                println!("  {url}");
                println!(
                    "  Or type this code in the Claude Code Notifier app:  {}",
                    issued.code
                );
            }
            None => println!(
                "  Enter this code in the Claude Code Notifier app:  {}",
                issued.code
            ),
        }
        println!();
        println!(
            "  Hooks are installed, so you already get desktop notifications on this computer."
        );
        println!(
            "  Pair when you like: this code lasts {}.",
            duration_text(issued.expires_in)
        );
        if no_wait {
            println!("  Pairing completes on its own when the phone connects.");
            println!();
            return Ok(PairOutcome::CodeShown);
        }
        println!(
            "  Waiting for your phone... (Ctrl+C is safe: pairing completes on its own when the phone connects)"
        );
        println!();
        let deadline = std::time::Instant::now() + Duration::from_secs(issued.expires_in);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(3));
            if let Ok(devices) = api.pairings()
                && let Some(d) = devices.iter().find(|d| !before.contains(&d.id))
            {
                print_paired(d.name.as_deref(), &d.platform);
                paired_with = Some((d.id.clone(), d.welcome_notification_id.clone()));
                break;
            }
        }
        if paired_with.is_none() {
            bail!("the code expired before a phone claimed it. Run `ccn pair` again.");
        }
    }

    cfg.client_id = Some(client_id);
    cfg.machine_name = Some(machine_name);
    cfg.registered_at = Some(config::now());
    // `ccn pair` has told the user itself, so the hooks needn't
    cfg.paired_message_shown = true;
    cfg.save()?;
    let store = config::store_token(&cfg, &token)?;
    Unpaired::clear();
    println!(
        "Credentials stored in the {store} ({}).",
        config::ccn_dir().display()
    );
    if list_phones(&mut cfg, &api).is_err() {
        // The server couldn't be asked for the count, but a phone did just pair
        cfg.record_phones(cfg.phones.unwrap_or(0).max(1) as usize);
        let _ = cfg.save();
    }
    sync_settings(&mut cfg, &api);

    match paired_with {
        Some((_, Some(welcome))) => wait_for_welcome(&api, &welcome),
        // An older server doesn't send a welcome, so send the plain test notification
        Some((device_id, None)) => match api.post_event(
            &payload::test_notification(&device_id),
            Duration::from_secs(10),
        ) {
            Ok(_) => println!("Sent a test notification to your phone."),
            Err(err) => println!("Could not send a test notification yet: {err:#}"),
        },
        None => {}
    }

    licence_pitch(&api);

    println!();
    println!(
        "Done. Start `claude` as usual: you will get a push when Claude finishes or needs you,"
    );
    println!("and you can approve, deny or answer from the phone. `ccn doctor` checks everything.");
    Ok(PairOutcome::Paired)
}

/// Pairs with the 6-digit code the phone shows. Prints the result.
fn claim_with_code(api: &Api, identity: &Value, code: &str) -> Result<PairedPhone> {
    let code = code.trim();
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        bail!("the code is the 6 digits shown in the Claude Code Notifier app");
    }
    let mut body = identity.clone();
    body["code"] = Value::String(code.to_string());
    let result = api.claim_by_client(&body)?;
    print_paired(result.device.name.as_deref(), &result.device.platform);
    Ok((result.device.id, result.welcome_notification_id))
}

fn print_paired(name: Option<&str>, platform: &str) {
    println!("Paired with {} ({platform}).", name.unwrap_or("your phone"));
}

/// The server didn't know the code (or it had expired), or it wasn't six digits
fn code_was_refused(err: &anyhow::Error) -> bool {
    matches!(
        telemetry::failure_reason(err),
        "invalid_code" | "malformed_code"
    )
}

/// After a refused code: true when the user pressed Enter for a QR code. Without a terminal there
/// is nobody to ask, so it prints both ways forward and says no.
fn offer_qr_instead() -> Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        println!("{OPTIONS_WITHOUT_TERMINAL}");
        return Ok(false);
    }
    println!();
    println!("{OFFER_QR}");
    let mut line = String::new();
    // End of input (Ctrl+D) isn't a yes
    Ok(matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0))
}

/// "24 hours", "1 hour", "15 minutes": how long a pairing code lasts.
pub fn duration_text(secs: u64) -> String {
    if secs >= 3600 {
        match secs / 3600 {
            1 => "1 hour".to_string(),
            h => format!("{h} hours"),
        }
    } else {
        match (secs / 60).max(1) {
            1 => "1 minute".to_string(),
            m => format!("{m} minutes"),
        }
    }
}

/// The paired phones, recording how many there are so the hooks know which mode they are in.
pub fn list_phones(cfg: &mut Config, api: &Api) -> Result<Vec<api::PairedDevice>> {
    let devices = api.pairings()?;
    cfg.record_phones(devices.len());
    let _ = cfg.save();
    Ok(devices)
}

/// After pairing, the one place the web licence is offered with its price. The server decides
/// whether there is an offer and what it costs; an older server sends none.
fn licence_pitch(api: &Api) {
    let Ok(status) = api.licence_status(Duration::from_secs(5)) else {
        return;
    };
    if status.licensed {
        return;
    }
    let Some(offer) = status.offer else {
        return;
    };
    println!();
    println!(
        "Your phone will offer a subscription in the app. You can also unlock everything from here, outside the app stores:"
    );
    println!("  ccn unlock             {} a year", offer.annual_price);
    println!("  ccn unlock --lifetime  {} once", offer.lifetime_price);
    println!(
        "Covers {} computers and every phone paired with them. {}-day money-back.",
        offer.computers, offer.money_back_days
    );
    telemetry::event("cli_pitch_shown", &[]);
}

/// The server pushed the phone an answerable welcome when it paired. Waiting here for the answer
/// closes the loop in front of the user: phone to terminal, the way every real answer will go.
fn wait_for_welcome(api: &Api, notification_id: &str) {
    use std::io::IsTerminal;
    println!();
    if !std::io::stdin().is_terminal() {
        println!("A test notification is on its way to your phone: try answering it there.");
        return;
    }
    println!("A test notification is on its way to your phone. Try answering it there...");
    println!("(press Enter to skip)");
    let (skip_tx, skip_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        // End of input isn't a key press
        if matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0) {
            let _ = skip_tx.send(());
        }
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    while std::time::Instant::now() < deadline {
        if skip_rx.try_recv().is_ok() {
            return;
        }
        match api.wait_response(notification_id, 3) {
            Ok(api::Wait::Responded(answer)) => {
                println!(
                    "✓ Your phone answered \"{}\". That is how you'll answer Claude.",
                    answer.trim()
                );
                return;
            }
            Ok(api::Wait::Settled(_)) => return,
            Ok(api::Wait::Pending) => {}
            Err(_) => std::thread::sleep(Duration::from_secs(3)),
        }
    }
    println!("No answer yet. The notification stays on your phone for an hour.");
}

/// `ccn` on its own: pair when no phone is paired with this computer yet (so `npx @touchtap/ccn` and a
/// fresh `brew install` go straight to the QR code), otherwise say where things stand.
pub fn default_command() -> Result<()> {
    ensure_setup(false)?;
    let mut cfg = Config::load();
    if !cfg.is_paired() {
        return pair(None, None, false, false);
    }
    // Registered without a phone (`ccn activate` or `ccn unlock` before pairing, or a QR code
    // nobody scanned yet): pairing is next
    let api = Api::new(&cfg);
    if matches!(list_phones(&mut cfg, &api), Ok(devices) if devices.is_empty()) {
        return pair(None, None, false, false);
    }
    println!(
        "ccn {} is set up on this computer as \"{}\".",
        config::VERSION,
        cfg.machine_name()
    );
    if let Some(line) = licence::status_line(&api) {
        println!("Licence: {line}");
    }
    println!();
    println!("  ccn doctor     check hooks, credentials and the server");
    println!("  ccn test       send a test notification to your phone");
    println!("  ccn pair       pair another phone");
    println!("  ccn --help     everything else");
    Ok(())
}

pub fn setup(replace_mac_app: bool) -> Result<()> {
    ensure_setup(replace_mac_app)
}

/// Installs the hooks and registers desktop notifications. Idempotent, and silent when everything
/// was already in place, so pairing and the install scripts can run it every time.
pub fn ensure_setup(replace_mac_app: bool) -> Result<()> {
    if let Some(copy) = hookcfg::install_self()? {
        println!("Installed ccn to {}.", copy.display());
    }
    let report = hookcfg::install(replace_mac_app)?;
    if report.changed {
        println!(
            "Hooks installed in {} (using {}).",
            report.path.display(),
            report.exe.display()
        );
    }
    if report.mac_app_hooks_found && !report.mac_app_hooks_removed {
        println!();
        println!(
            "Note: the Claude Code Notifier Mac app's hooks are also installed, so each event"
        );
        println!(
            "would be sent twice. Run `ccn setup --replace-mac-app` to remove them, or quit the Mac app."
        );
    } else if report.mac_app_hooks_removed {
        println!("Removed the Mac app's hook entries; ccn handles notifications from now on.");
    }
    if let Some(line) = notify::install() {
        println!("{line}");
    }
    Ok(())
}

fn sync_settings(cfg: &mut Config, api: &Api) {
    match api.get_settings(None, Duration::from_secs(10)) {
        Ok(Some(result)) => {
            cfg.settings = result.settings;
            cfg.settings_updated_at = result.updated_at;
            cfg.settings_etag = result.etag;
            cfg.settings_fetched_at = Some(config::now());
            let _ = cfg.save();
        }
        Ok(None) => {}
        Err(err) => println!("Settings not synced yet: {err:#}"),
    }
}

pub fn sync() -> Result<()> {
    let mut cfg = Config::load();
    require_paired(&cfg)?;
    let api = Api::new(&cfg);
    sync_settings(&mut cfg, &api);
    println!("Settings synced.");
    config_show()
}

fn require_paired(cfg: &Config) -> Result<()> {
    if !cfg.is_paired() {
        bail!("this computer is not paired yet. Run `ccn pair <code>` with the code from the app.");
    }
    Ok(())
}

pub fn doctor() -> Result<()> {
    let mut cfg = Config::load();
    let yes = "ok ";
    let no = "!! ";
    println!("ccn {} on {}", config::VERSION, config::platform());
    println!("{}config: {}", yes, config::config_path().display());

    let paired = cfg.is_paired();
    println!(
        "{}credentials: {}",
        if paired { yes } else { no },
        if paired {
            format!(
                "present ({})",
                if config::credential_store(&cfg) == "keychain" {
                    "keychain"
                } else {
                    "file"
                }
            )
        } else {
            "missing: run `ccn pair <code>`".to_string()
        }
    );

    let api = Api::new(&cfg);
    match api.health() {
        Ok(()) => println!("{}server: {} reachable", yes, cfg.server_url),
        Err(err) => println!("{}server: {err:#}", no),
    }
    let mut accepted = paired;
    if paired {
        match api.me() {
            Ok(me) => {
                Unpaired::clear();
                println!(
                    "{}registered as \"{}\" ({} {}), capabilities: {}",
                    yes,
                    me.name,
                    me.client_kind.unwrap_or_else(|| "cli".to_string()),
                    me.platform
                        .unwrap_or_else(|| config::platform().to_string()),
                    me.capabilities.unwrap_or_default().join(", ")
                )
            }
            Err(err) if api::is_rejected(&err) => {
                if Unpaired::load().is_none() {
                    Unpaired::new().save();
                }
                println!("{}pairing: not paired any more: run `ccn pair`", no);
                accepted = false;
            }
            Err(err) => {
                if Unpaired::load().is_some() {
                    println!("{}pairing: not paired any more: run `ccn pair`", no);
                    accepted = false;
                }
                println!("{}registration: {err:#}", no);
            }
        }
    }
    if accepted {
        if let Some(line) = licence::status_line(&api) {
            println!(
                "{}licence: {line}",
                if line.ends_with("ccn upgrade") || line.ends_with("ccn unlock") {
                    no
                } else {
                    yes
                }
            );
        }
        match list_phones(&mut cfg, &api) {
            Ok(devices) if devices.is_empty() => println!(
                "{}phones: none paired yet, so notifications stay on this desktop (run `ccn pair`)",
                no
            ),
            Ok(devices) => {
                let names: Vec<String> = devices
                    .iter()
                    .map(|d| {
                        format!(
                            "{} ({})",
                            d.name.clone().unwrap_or_else(|| d.id.clone()),
                            d.platform
                        )
                    })
                    .collect();
                println!("{}phones: {}", yes, names.join(", "));
            }
            Err(err) => println!("{}phones: {err:#}", no),
        }
    }

    match hookcfg::status() {
        Ok(status) => {
            if status.missing.is_empty() {
                println!(
                    "{}hooks: all {} installed in {}",
                    yes,
                    status.installed.len(),
                    status.path.display()
                );
            } else {
                println!(
                    "{}hooks: missing {} (run `ccn setup`)",
                    no,
                    status.missing.join(", ")
                );
            }
            if status.stale_exe {
                println!(
                    "{}hooks point at a different ccn binary; run `ccn setup` to update them",
                    no
                );
            }
            if status.mac_app_present {
                println!(
                    "{}the Mac app's hooks are also installed (run `ccn setup --replace-mac-app`)",
                    no
                );
            }
        }
        Err(err) => println!("{}hooks: {err:#}", no),
    }

    match presence::idle_seconds() {
        Some(idle) => println!(
            "{}presence: idle {:.0}s now, threshold {:.0}s",
            yes,
            idle,
            cfg.settings.presence_threshold()
        ),
        None => println!(
            "{}presence: not detectable here, so every request waits for the phone",
            no
        ),
    }
    let tmux = std::env::var("TMUX")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    println!(
        "{}replies: approvals and question answers from the phone work in any terminal; free text into an idle session {}",
        yes,
        if tmux { "works (tmux)" } else { "needs tmux" }
    );
    if let Some((ok, line)) = notify::doctor_line() {
        println!("{}desktop notifications: {line}", if ok { yes } else { no });
    }
    println!("{}log: {}", yes, config::log_path().display());
    Ok(())
}

pub fn test() -> Result<()> {
    let mut cfg = Config::load();
    require_paired(&cfg)?;
    let api = Api::new(&cfg);
    let devices = list_phones(&mut cfg, &api)?;
    if devices.is_empty() {
        bail!("no phones are paired. Run `ccn pair <code>`.");
    }
    for d in devices {
        let name = d.name.clone().unwrap_or_else(|| d.id.clone());
        match api.post_event(&payload::test_notification(&d.id), Duration::from_secs(10)) {
            Ok(_) => println!("Sent a test notification to {name} ({}).", d.platform),
            Err(err) => println!("{name}: {err:#}"),
        }
    }
    Ok(())
}

pub fn devices() -> Result<()> {
    let mut cfg = Config::load();
    require_paired(&cfg)?;
    let api = Api::new(&cfg);
    let devices = list_phones(&mut cfg, &api)?;
    if devices.is_empty() {
        println!("No phones paired. Run `ccn pair <code>` with the code from the app.");
        return Ok(());
    }
    for d in devices {
        let since = chrono::DateTime::from_timestamp(d.created_at, 0)
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        println!(
            "{:<24} {:<8} paired {}{}",
            d.name.clone().unwrap_or_else(|| d.id.clone()),
            d.platform,
            since,
            if d.notifications_enabled == Some(false) {
                "  (notifications off)"
            } else {
                ""
            }
        );
    }
    Ok(())
}

pub fn revoke(device: &str) -> Result<()> {
    let mut cfg = Config::load();
    require_paired(&cfg)?;
    let api = Api::new(&cfg);
    let devices = list_phones(&mut cfg, &api)?;
    let target = devices
        .iter()
        .find(|d| {
            d.id == device
                || d.name
                    .as_deref()
                    .map(|n| n.eq_ignore_ascii_case(device))
                    .unwrap_or(false)
        })
        .with_context(|| format!("no paired phone called '{device}'"))?;
    api.revoke(&target.pairing_id)?;
    println!(
        "Revoked {}.",
        target.name.clone().unwrap_or_else(|| target.id.clone())
    );
    let _ = list_phones(&mut cfg, &api);
    Ok(())
}

pub fn config_show() -> Result<()> {
    let cfg = Config::load();
    let s = &cfg.settings;
    println!("notify_on_done             {}", s.notify_on_done());
    println!("notify_on_permission       {}", s.notify_on_permission());
    println!("notify_on_question         {}", s.notify_on_question());
    println!(
        "quiet_hours                {}",
        match &s.quiet_hours {
            Some(q) if q.enabled => format!("{}-{}", q.start, q.end),
            _ => "off".to_string(),
        }
    );
    println!("hold_ceiling_seconds       {}", s.hold_ceiling());
    println!(
        "presence_threshold_seconds {}",
        s.presence_threshold() as u64
    );
    println!("stop_hold_seconds          {}", s.stop_hold());
    println!(
        "reminder_interval_seconds  {}",
        s.reminder_interval_seconds.unwrap_or(0)
    );
    println!("sound                      {}", s.sound());
    println!("quiet_when_present         {}", s.quiet_when_present());
    println!(
        "desktop_notifications      {} (local only)",
        cfg.desktop_notifications
    );
    println!(
        "credential_store           {} (local only)",
        config::credential_store(&cfg)
    );
    println!(
        "pair_nudges                {} (local only)",
        if cfg.pair_nudges { "on" } else { "off" }
    );
    if let Some(at) = cfg.settings_updated_at {
        let when = chrono::DateTime::from_timestamp(at, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();
        println!("(last changed {when}; also editable from the phone)");
    }
    Ok(())
}

pub fn config_set(key: &str, value: &str) -> Result<()> {
    let mut cfg = Config::load();
    if key == "desktop_notifications" {
        cfg.desktop_notifications = matches!(
            value.to_ascii_lowercase().as_str(),
            "on" | "true" | "yes" | "1"
        );
        cfg.save()?;
        return config_show();
    }
    if key == "pair_nudges" {
        cfg.pair_nudges = matches!(
            value.to_ascii_lowercase().as_str(),
            "on" | "true" | "yes" | "1"
        );
        cfg.save()?;
        return config_show();
    }
    if key == "credential_store" {
        let store = value.to_ascii_lowercase();
        anyhow::ensure!(
            store == "keychain" || store == "file",
            "credential_store expects keychain or file"
        );
        // Move the token: read it from wherever it is now, then store it in the chosen place
        let token = config::load_token(&cfg);
        cfg.credential_store = store;
        cfg.credential_store_chosen = true;
        cfg.save()?;
        if let Some(token) = token {
            if config::credential_store(&cfg) == "file" {
                config::delete_token();
            }
            let went = config::store_token(&cfg, &token)?;
            println!("Credentials are now in the {went}.");
        }
        return config_show();
    }
    let mut settings = cfg.settings.clone();
    settings.set_from_str(key, value)?;
    if cfg.is_paired() {
        let result = Api::new(&cfg).put_settings(&settings)?;
        cfg.settings = result.settings;
        cfg.settings_updated_at = result.updated_at;
        cfg.settings_etag = result.etag;
        cfg.settings_fetched_at = Some(config::now());
    } else {
        cfg.settings = settings;
    }
    cfg.save()?;
    config_show()
}

pub fn uninstall() -> Result<()> {
    remove_local(true)
}

/// Erases this computer from the server, then removes ccn exactly as `ccn uninstall` does. The
/// server goes first: if it can't be reached the credentials stay, so the user can try again.
pub fn forget(yes: bool) -> Result<()> {
    use std::io::{IsTerminal, Write};
    if !yes && !std::io::stdin().is_terminal() {
        bail!("ccn forget needs a yes from you: run it in a terminal, or add --yes");
    }
    println!(
        "This erases this computer and its pairings from the Claude Code Notifier server, then"
    );
    println!(
        "removes ccn from Claude Code and deletes its credentials and files here, as `ccn uninstall` does."
    );
    println!("Phones paired with this computer will see it disappear.");
    println!(
        "A licence bought on the website is not cancelled: do that at claudecodenotifier.com/licence"
    );
    println!("with \"Erase my licence\".");
    if !yes {
        print!("\nType yes to continue: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != "yes" {
            println!("Nothing was erased.");
            return Ok(());
        }
    }
    let cfg = Config::load();
    if cfg.is_paired() {
        match Api::new(&cfg).erase() {
            Ok(()) => println!("Erased this computer and its pairings from the server."),
            Err(err) if api::is_rejected(&err) => {
                println!("The server had already forgotten this computer.")
            }
            Err(err) => bail!(
                "{err:#}. Nothing was removed from this computer, so you can run `ccn forget` again"
            ),
        }
    } else {
        println!(
            "This computer isn't registered with the server, so there is nothing to erase there."
        );
    }
    // The server has already dropped the licence slot and the pairings
    remove_local(false)
}

/// Removes the hooks, credentials and local files. `revoke_on_server` also frees the licence slot
/// and unpairs every phone first, which `forget` has already done by erasing the computer.
fn remove_local(revoke_on_server: bool) -> Result<()> {
    let cfg = Config::load();
    let path = hookcfg::uninstall()?;
    println!("Removed ccn's hooks from {}.", path.display());
    notify::uninstall();
    if revoke_on_server && cfg.is_paired() {
        let api = Api::new(&cfg);
        // Free the licence slot first, so a reinstall (which is a new computer) doesn't find it taken
        if let Ok(true) = api.deactivate_licence() {
            println!("Freed this computer's licence slot.");
        }
        if let Ok(devices) = api.pairings() {
            for d in devices {
                let _ = api.revoke(&d.pairing_id);
            }
            println!("Unpaired every phone.");
        }
    }
    config::delete_token();
    let _ = std::fs::remove_dir_all(config::ccn_dir());
    println!("Removed {}.", config::ccn_dir().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_lifetimes_read_naturally() {
        assert_eq!(duration_text(24 * 3600), "24 hours");
        assert_eq!(duration_text(3600), "1 hour");
        assert_eq!(duration_text(90 * 60), "1 hour");
        assert_eq!(duration_text(15 * 60), "15 minutes");
        assert_eq!(duration_text(60), "1 minute");
        assert_eq!(duration_text(30), "1 minute");
    }

    #[test]
    fn only_a_refused_code_offers_the_qr_code() {
        let refused = anyhow::anyhow!("pairing: Invalid or expired pairing code");
        let malformed =
            anyhow::anyhow!("the code is the 6 digits shown in the Claude Code Notifier app");
        let network = anyhow::anyhow!("POST /api/pairing/claim-by-client: io: Connection refused");
        let limited = anyhow::anyhow!("pairing: Too many pairing attempts");
        assert!(code_was_refused(&refused));
        assert!(code_was_refused(&malformed));
        assert!(!code_was_refused(&network));
        assert!(!code_was_refused(&limited));
    }
}
