//! The user-facing commands: pair, setup, doctor, test, devices, config, sync, uninstall, forget.

use anyhow::{Context, Result, bail};
use rust_i18n::t;
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

/// A pairing that failed on this side, before or without the server. Typed, so telemetry can name
/// the reason whatever language the message is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairFailure {
    /// The code typed wasn't six digits
    MalformedCode,
    /// The code shown here expired before a phone claimed it
    CodeTimeout,
}

impl std::fmt::Display for PairFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            PairFailure::MalformedCode => t!("pair.code_is_six_digits"),
            PairFailure::CodeTimeout => t!("pair.code_expired"),
        })
    }
}

impl std::error::Error for PairFailure {}

/// "keychain" or "file" as a word in a sentence.
pub fn store_name(store: &str) -> String {
    if store == "keychain" {
        t!("common.store_keychain").into_owned()
    } else {
        t!("common.store_file").into_owned()
    }
}

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
        println!("{}", t!("pair.reregister"));
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
                println!("{}", t!("pair.code_not_recognised"));
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
                println!("{}", t!("pair.scan"));
                println!();
                print!("{code}");
                println!();
                println!("  {url}");
                println!("{}", t!("pair.or_type_code", code = issued.code));
            }
            None => println!("{}", t!("pair.enter_code", code = issued.code)),
        }
        println!();
        println!("{}", t!("pair.hooks_already_installed"));
        println!(
            "{}",
            t!(
                "pair.code_lasts",
                duration = duration_text(issued.expires_in)
            )
        );
        if no_wait {
            println!("{}", t!("pair.completes_on_its_own"));
            println!();
            return Ok(PairOutcome::CodeShown);
        }
        println!("{}", t!("pair.waiting"));
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
            bail!(PairFailure::CodeTimeout);
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
        "{}",
        t!(
            "pair.credentials_stored",
            store = store_name(store),
            path = config::ccn_dir().display()
        )
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
            Ok(_) => println!("{}", t!("pair.test_sent")),
            Err(err) => println!("{}", t!("pair.test_failed", error = format!("{err:#}"))),
        },
        None => {}
    }

    licence_pitch(&api);

    println!();
    println!("{}", t!("pair.done"));
    Ok(PairOutcome::Paired)
}

/// Pairs with the 6-digit code the phone shows. Prints the result.
fn claim_with_code(api: &Api, identity: &Value, code: &str) -> Result<PairedPhone> {
    let code = code.trim();
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        bail!(PairFailure::MalformedCode);
    }
    let mut body = identity.clone();
    body["code"] = Value::String(code.to_string());
    let result = api.claim_by_client(&body)?;
    print_paired(result.device.name.as_deref(), &result.device.platform);
    Ok((result.device.id, result.welcome_notification_id))
}

fn print_paired(name: Option<&str>, platform: &str) {
    let your_phone = t!("common.your_phone");
    println!(
        "{}",
        t!(
            "pair.paired_with",
            name = name.unwrap_or(&your_phone),
            platform = platform
        )
    );
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
        println!("{}", t!("pair.options_without_terminal"));
        return Ok(false);
    }
    println!();
    println!("{}", t!("pair.offer_qr"));
    let mut line = String::new();
    // End of input (Ctrl+D) isn't a yes
    Ok(matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0))
}

/// "24 hours", "1 hour", "15 minutes": how long a pairing code lasts.
pub fn duration_text(secs: u64) -> String {
    if secs >= 3600 {
        match secs / 3600 {
            1 => t!("pair.hours_one").into_owned(),
            h => t!("pair.hours_other", count = h).into_owned(),
        }
    } else {
        match (secs / 60).max(1) {
            1 => t!("pair.minutes_one").into_owned(),
            m => t!("pair.minutes_other", count = m).into_owned(),
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
    println!("{}", t!("pair.pitch"));
    println!("{}", t!("pair.pitch_annual", price = offer.annual_price));
    println!(
        "{}",
        t!("pair.pitch_lifetime", price = offer.lifetime_price)
    );
    println!(
        "{}",
        t!(
            "pair.pitch_covers",
            computers = offer.computers,
            days = offer.money_back_days
        )
    );
    telemetry::event("cli_pitch_shown", &[]);
}

/// The server pushed the phone an answerable welcome when it paired. Waiting here for the answer
/// closes the loop in front of the user: phone to terminal, the way every real answer will go.
fn wait_for_welcome(api: &Api, notification_id: &str) {
    use std::io::IsTerminal;
    println!();
    if !std::io::stdin().is_terminal() {
        println!("{}", t!("pair.welcome_no_terminal"));
        return;
    }
    println!("{}", t!("pair.welcome"));
    println!("{}", t!("pair.welcome_skip"));
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
                println!("{}", t!("pair.welcome_answered", answer = answer.trim()));
                return;
            }
            Ok(api::Wait::Settled(_)) => return,
            Ok(api::Wait::Pending) => {}
            Err(_) => std::thread::sleep(Duration::from_secs(3)),
        }
    }
    println!("{}", t!("pair.welcome_no_answer"));
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
        "{}",
        t!(
            "status.set_up",
            version = config::VERSION,
            name = cfg.machine_name()
        )
    );
    if let Some((_, line)) = licence::status_line(&api) {
        println!("{}", t!("status.licence", line = line));
    }
    println!();
    println!("{}", t!("status.hint_doctor"));
    println!("{}", t!("status.hint_test"));
    println!("{}", t!("status.hint_pair"));
    println!("{}", t!("status.hint_help"));
    Ok(())
}

pub fn setup(replace_mac_app: bool) -> Result<()> {
    ensure_setup(replace_mac_app)
}

/// Installs the hooks and registers desktop notifications. Idempotent, and silent when everything
/// was already in place, so pairing and the install scripts can run it every time.
pub fn ensure_setup(replace_mac_app: bool) -> Result<()> {
    if let Some(copy) = hookcfg::install_self()? {
        println!("{}", t!("setup.installed_self", path = copy.display()));
    }
    let report = hookcfg::install(replace_mac_app)?;
    if report.changed {
        println!(
            "{}",
            t!(
                "setup.hooks_installed",
                path = report.path.display(),
                exe = report.exe.display()
            )
        );
    }
    if report.mac_app_hooks_found && !report.mac_app_hooks_removed {
        println!();
        println!("{}", t!("setup.mac_app_note"));
    } else if report.mac_app_hooks_removed {
        println!("{}", t!("setup.mac_app_removed"));
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
        Err(err) => println!("{}", t!("sync.not_synced", error = format!("{err:#}"))),
    }
}

pub fn sync() -> Result<()> {
    let mut cfg = Config::load();
    require_paired(&cfg)?;
    let api = Api::new(&cfg);
    sync_settings(&mut cfg, &api);
    println!("{}", t!("sync.synced"));
    config_show()
}

fn require_paired(cfg: &Config) -> Result<()> {
    if !cfg.is_paired() {
        bail!("{}", t!("common.not_paired"));
    }
    Ok(())
}

pub fn doctor() -> Result<()> {
    let mut cfg = Config::load();
    let yes = "ok ";
    let no = "!! ";
    println!(
        "{}",
        t!(
            "doctor.header",
            version = config::VERSION,
            platform = config::platform()
        )
    );
    println!(
        "{yes}{}",
        t!("doctor.config", path = config::config_path().display())
    );

    let paired = cfg.is_paired();
    if paired {
        println!(
            "{yes}{}",
            t!(
                "doctor.credentials_present",
                store = store_name(config::credential_store(&cfg))
            )
        );
    } else {
        println!("{no}{}", t!("doctor.credentials_missing"));
    }

    let api = Api::new(&cfg);
    match api.health() {
        Ok(()) => println!(
            "{yes}{}",
            t!("doctor.server_reachable", url = cfg.server_url)
        ),
        Err(err) => println!(
            "{no}{}",
            t!("doctor.server_error", error = format!("{err:#}"))
        ),
    }
    let mut accepted = paired;
    if paired {
        match api.me() {
            Ok(me) => {
                Unpaired::clear();
                println!(
                    "{yes}{}",
                    t!(
                        "doctor.registered",
                        name = me.name,
                        kind = me.client_kind.unwrap_or_else(|| "cli".to_string()),
                        platform = me
                            .platform
                            .unwrap_or_else(|| config::platform().to_string()),
                        capabilities = me.capabilities.unwrap_or_default().join(", ")
                    )
                )
            }
            Err(err) if api::is_rejected(&err) => {
                if Unpaired::load().is_none() {
                    Unpaired::new().save();
                }
                println!("{no}{}", t!("doctor.not_paired_any_more"));
                accepted = false;
            }
            Err(err) => {
                if Unpaired::load().is_some() {
                    println!("{no}{}", t!("doctor.not_paired_any_more"));
                    accepted = false;
                }
                println!(
                    "{no}{}",
                    t!("doctor.registration_error", error = format!("{err:#}"))
                );
            }
        }
    }
    if accepted {
        if let Some((licensed, line)) = licence::status_line(&api) {
            println!(
                "{}{}",
                if licensed { yes } else { no },
                t!("doctor.licence", line = line)
            );
        }
        match list_phones(&mut cfg, &api) {
            Ok(devices) if devices.is_empty() => {
                println!("{no}{}", t!("doctor.phones_none"))
            }
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
                println!("{yes}{}", t!("doctor.phones", names = names.join(", ")));
            }
            Err(err) => println!(
                "{no}{}",
                t!("doctor.phones_error", error = format!("{err:#}"))
            ),
        }
    }

    match hookcfg::status() {
        Ok(status) => {
            if status.missing.is_empty() {
                println!(
                    "{yes}{}",
                    t!(
                        "doctor.hooks_all",
                        count = status.installed.len(),
                        path = status.path.display()
                    )
                );
            } else {
                println!(
                    "{no}{}",
                    t!("doctor.hooks_missing", names = status.missing.join(", "))
                );
            }
            if status.stale_exe {
                println!("{no}{}", t!("doctor.hooks_stale"));
            }
            if status.mac_app_present {
                println!("{no}{}", t!("doctor.mac_app_hooks"));
            }
        }
        Err(err) => println!(
            "{no}{}",
            t!("doctor.hooks_error", error = format!("{err:#}"))
        ),
    }

    match presence::idle_seconds() {
        Some(idle) => println!(
            "{yes}{}",
            t!(
                "doctor.presence",
                idle = format!("{idle:.0}"),
                threshold = format!("{:.0}", cfg.settings.presence_threshold())
            )
        ),
        None => println!("{no}{}", t!("doctor.presence_none")),
    }
    let tmux = std::env::var("TMUX")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    println!(
        "{yes}{}",
        t!(
            "doctor.replies",
            free_text = if tmux {
                t!("doctor.replies_tmux")
            } else {
                t!("doctor.replies_no_tmux")
            }
        )
    );
    if let Some((ok, line)) = notify::doctor_line() {
        println!(
            "{}{}",
            if ok { yes } else { no },
            t!("doctor.desktop", line = line)
        );
    }
    println!(
        "{yes}{}",
        t!("doctor.log", path = config::log_path().display())
    );
    Ok(())
}

pub fn test() -> Result<()> {
    let mut cfg = Config::load();
    require_paired(&cfg)?;
    let api = Api::new(&cfg);
    let devices = list_phones(&mut cfg, &api)?;
    if devices.is_empty() {
        bail!("{}", t!("test.none_paired"));
    }
    for d in devices {
        let name = d.name.clone().unwrap_or_else(|| d.id.clone());
        match api.post_event(&payload::test_notification(&d.id), Duration::from_secs(10)) {
            Ok(_) => println!("{}", t!("test.sent", name = name, platform = d.platform)),
            Err(err) => println!(
                "{}",
                t!("test.failed", name = name, error = format!("{err:#}"))
            ),
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
        println!("{}", t!("devices.none"));
        return Ok(());
    }
    for d in devices {
        let since = chrono::DateTime::from_timestamp(d.created_at, 0)
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        println!(
            "{}",
            t!(
                "devices.row",
                name = format!("{:<24}", d.name.clone().unwrap_or_else(|| d.id.clone())),
                platform = format!("{:<8}", d.platform),
                since = since,
                off = if d.notifications_enabled == Some(false) {
                    t!("devices.notifications_off")
                } else {
                    "".into()
                }
            )
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
        .with_context(|| t!("devices.no_such_phone", device = device).into_owned())?;
    api.revoke(&target.pairing_id)?;
    println!(
        "{}",
        t!(
            "devices.revoked",
            name = target.name.clone().unwrap_or_else(|| target.id.clone())
        )
    );
    let _ = list_phones(&mut cfg, &api);
    Ok(())
}

pub fn config_show() -> Result<()> {
    let cfg = Config::load();
    let s = &cfg.settings;
    let local_only = t!("config.local_only");
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
        "desktop_notifications      {} {local_only}",
        cfg.desktop_notifications
    );
    println!(
        "credential_store           {} {local_only}",
        config::credential_store(&cfg)
    );
    println!(
        "pair_nudges                {} {local_only}",
        if cfg.pair_nudges { "on" } else { "off" }
    );
    if let Some(at) = cfg.settings_updated_at {
        let when = chrono::DateTime::from_timestamp(at, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();
        println!("{}", t!("config.last_changed", when = when));
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
            "{}",
            t!("config.credential_store_expects")
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
            println!(
                "{}",
                t!("config.credentials_moved", store = store_name(went))
            );
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
        bail!("{}", t!("forget.needs_yes"));
    }
    println!("{}", t!("forget.warning"));
    if !yes {
        let yes_word = t!("forget.yes_word");
        print!("{}", t!("forget.prompt", yes = yes_word));
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        // The English word always works too, whatever the prompt said
        if line.trim() != yes_word && line.trim() != "yes" {
            println!("{}", t!("forget.nothing_erased"));
            return Ok(());
        }
    }
    let cfg = Config::load();
    if cfg.is_paired() {
        match Api::new(&cfg).erase() {
            Ok(()) => println!("{}", t!("forget.erased")),
            Err(err) if api::is_rejected(&err) => {
                println!("{}", t!("forget.already_forgotten"))
            }
            Err(err) => bail!(
                "{}",
                t!("forget.nothing_removed", error = format!("{err:#}"))
            ),
        }
    } else {
        println!("{}", t!("forget.not_registered"));
    }
    // The server has already dropped the licence slot and the pairings
    remove_local(false)
}

/// Removes the hooks, credentials and local files. `revoke_on_server` also frees the licence slot
/// and unpairs every phone first, which `forget` has already done by erasing the computer.
fn remove_local(revoke_on_server: bool) -> Result<()> {
    let cfg = Config::load();
    let path = hookcfg::uninstall()?;
    println!("{}", t!("uninstall.hooks_removed", path = path.display()));
    notify::uninstall();
    if revoke_on_server && cfg.is_paired() {
        let api = Api::new(&cfg);
        // Free the licence slot first, so a reinstall (which is a new computer) doesn't find it taken
        if let Ok(true) = api.deactivate_licence() {
            println!("{}", t!("uninstall.slot_freed"));
        }
        if let Ok(devices) = api.pairings() {
            for d in devices {
                let _ = api.revoke(&d.pairing_id);
            }
            println!("{}", t!("uninstall.unpaired_all"));
        }
    }
    config::delete_token();
    let _ = std::fs::remove_dir_all(config::ccn_dir());
    println!(
        "{}",
        t!("uninstall.removed_dir", path = config::ccn_dir().display())
    );
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
        let malformed = anyhow::anyhow!(PairFailure::MalformedCode);
        let network = anyhow::anyhow!("POST /api/pairing/claim-by-client: io: Connection refused");
        let limited = anyhow::anyhow!("pairing: Too many pairing attempts");
        assert!(code_was_refused(&refused));
        assert!(code_was_refused(&malformed));
        assert!(!code_was_refused(&network));
        assert!(!code_was_refused(&limited));
    }

    #[test]
    fn pair_failures_read_as_before() {
        assert_eq!(
            format!("{:#}", anyhow::anyhow!(PairFailure::MalformedCode)),
            "the code is the 6 digits shown in the Claude Code Notifier app"
        );
        assert_eq!(
            format!("{:#}", anyhow::anyhow!(PairFailure::CodeTimeout)),
            "the code expired before a phone claimed it. Run `ccn pair` again."
        );
    }
}
