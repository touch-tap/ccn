//! ccn: Claude Code Notifier for the terminal.
//!
//! One binary, invoked by Claude Code's hooks. It runs for milliseconds and exits: there is no
//! daemon, no socket and no window. `ccn pair` installs the hooks once; after that every
//! `claude` session is covered.

mod api;
mod commands;
mod config;
mod hook;
mod hookcfg;
mod i18n;
mod licence;
mod live;
mod local;
mod logger;
mod notify;
mod payload;
mod presence;
mod qr;
mod respond;
mod session;
mod telemetry;
mod update;

use clap::{Arg, ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand};
use rust_i18n::t;

// Every string a person reads lives in locales/<lang>.yml; an unknown language reads English
rust_i18n::i18n!("locales", fallback = "en");

// The help text is read at `parse_cli()`, after `i18n::init` has chosen the language. clap has no
// translations of its own, so `localise` swaps its headings, -h/-V and `help` for translated ones;
// only parse errors keep clap's English message, with a translated hint underneath.
#[derive(Parser)]
#[command(name = "ccn", version, about = t!("cli.about").to_string())]
struct Cli {
    // With no command: pair this computer if it isn't yet, otherwise show its status
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = t!("cli.pair.about").to_string())]
    Pair {
        #[arg(help = t!("cli.pair.code").to_string())]
        code: Option<String>,
        #[arg(long, help = t!("cli.pair.name").to_string())]
        name: Option<String>,
        #[arg(long, help = t!("cli.pair.replace_mac_app").to_string())]
        replace_mac_app: bool,
        #[arg(long, help = t!("cli.pair.no_wait").to_string())]
        no_wait: bool,
    },
    #[command(about = t!("cli.setup.about").to_string())]
    Setup {
        #[arg(long, help = t!("cli.setup.replace_mac_app").to_string())]
        replace_mac_app: bool,
    },
    #[command(about = t!("cli.doctor.about").to_string())]
    Doctor,
    #[command(about = t!("cli.test.about").to_string())]
    Test,
    #[command(about = t!("cli.devices.about").to_string())]
    Devices {
        #[command(subcommand)]
        action: Option<DevicesCommand>,
    },
    #[command(about = t!("cli.config.about").to_string())]
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    #[command(about = t!("cli.sync.about").to_string())]
    Sync,
    #[command(alias = "buy", about = t!("cli.unlock.about").to_string())]
    Unlock {
        #[arg(long, help = t!("cli.unlock.lifetime").to_string())]
        lifetime: bool,
    },
    #[command(alias = "update", about = t!("cli.upgrade.about").to_string())]
    Upgrade {
        #[arg(long, help = t!("cli.upgrade.check").to_string())]
        check: bool,
        #[arg(long, help = t!("cli.upgrade.quiet").to_string())]
        quiet: bool,
        // Started by the daily check: never prints, logs to ccn.log instead
        #[arg(long, hide = true)]
        background: bool,
    },
    #[command(about = t!("cli.activate.about").to_string())]
    Activate {
        #[arg(help = t!("cli.activate.key").to_string())]
        key: String,
        #[arg(long, help = t!("cli.activate.replace").to_string())]
        replace: Option<String>,
    },
    #[command(about = t!("cli.deactivate.about").to_string())]
    Deactivate,
    #[command(alias = "license", about = t!("cli.licence.about").to_string())]
    Licence,
    #[command(about = t!("cli.uninstall.about").to_string())]
    Uninstall,
    #[command(about = t!("cli.forget.about").to_string())]
    Forget {
        #[arg(short, long, help = t!("cli.forget.yes").to_string())]
        yes: bool,
    },
    // Replaces clap's built-in `help` subcommand, whose description can't be translated
    #[command(about = t!("clap.help_command").to_string())]
    Help {
        #[arg(help = t!("clap.help_command_arg").to_string())]
        command: Vec<String>,
    },
    // Run as a Claude Code hook: reads the event JSON on stdin
    #[command(hide = true)]
    Hook {
        // Called by the Claude Code plugin, which stands aside when settings.json has ccn's hooks too
        #[arg(long)]
        plugin: bool,
    },
}

#[derive(Subcommand)]
enum DevicesCommand {
    #[command(about = t!("cli.devices.revoke").to_string())]
    Revoke { device: String },
}

#[derive(Subcommand)]
enum ConfigCommand {
    #[command(about = t!("cli.config.show").to_string())]
    Show,
    #[command(about = t!("cli.config.set").to_string())]
    Set { key: String, value: String },
}

fn main() {
    i18n::init();
    update::remove_leftovers();
    let cli = parse_cli(std::env::args_os());
    let Some(command) = cli.command else {
        if let Err(err) = commands::default_command() {
            report(&err);
            std::process::exit(1);
        }
        return;
    };
    let result = match command {
        // The plugin's stand-aside check needs the event's cwd, so hook.rs makes it
        Command::Hook { plugin } => hook::run(plugin),
        Command::Pair {
            code,
            name,
            replace_mac_app,
            no_wait,
        } => commands::pair(code, name, replace_mac_app, no_wait),
        Command::Setup { replace_mac_app } => commands::setup(replace_mac_app),
        Command::Doctor => commands::doctor(),
        Command::Test => commands::test(),
        Command::Devices { action } => match action {
            None => commands::devices(),
            Some(DevicesCommand::Revoke { device }) => commands::revoke(&device),
        },
        Command::Config { action } => match action {
            ConfigCommand::Show => commands::config_show(),
            // Local to this computer and owned by update.rs, so commands.rs needs no change
            ConfigCommand::Set { key, value } if key == "auto_update" => {
                update::set_auto_update(&value)
            }
            ConfigCommand::Set { key, value } => commands::config_set(&key, &value),
        },
        Command::Sync => commands::sync(),
        Command::Unlock { lifetime } => licence::unlock(lifetime),
        Command::Upgrade {
            check,
            quiet,
            background,
        } => update::run(update::Mode {
            check,
            quiet,
            background,
        }),
        Command::Activate { key, replace } => licence::activate(&key, replace),
        Command::Deactivate => licence::deactivate(),
        Command::Licence => licence::show(),
        Command::Uninstall => commands::uninstall(),
        Command::Forget { yes } => commands::forget(yes),
        // `ccn help pair` is `ccn pair --help`, which prints and exits
        Command::Help { command } => {
            let args = ["ccn".to_string()]
                .into_iter()
                .chain(command)
                .chain(["--help".to_string()]);
            parse_cli(args);
            Ok(())
        }
    };
    if let Err(err) = result {
        report(&err);
        std::process::exit(1);
    }
}

/// Parses the command line with the translated help. Help and version print and exit 0; a parse
/// error prints clap's message, then a translated hint, and exits 2.
fn parse_cli<I, T>(args: I) -> Cli
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let fail = |err: clap::Error| -> ! {
        use clap::error::ErrorKind;
        if !matches!(
            err.kind(),
            ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        ) {
            let _ = err.print();
            eprintln!("\n{}", t!("clap.error_hint"));
            std::process::exit(err.exit_code());
        }
        err.exit()
    };
    let matches = localise(Cli::command(), true)
        .try_get_matches_from(args)
        .unwrap_or_else(|err| fail(err));
    Cli::from_arg_matches(&matches).unwrap_or_else(|err| fail(err))
}

/// Gives a command, and every subcommand under it, translated help headings and -h/-V flags in
/// place of clap's English ones.
fn localise(cmd: clap::Command, top: bool) -> clap::Command {
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    let mut cmd = cmd
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help(t!("clap.help").to_string()),
        );
    if top {
        cmd = cmd.disable_version_flag(true).arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help(t!("clap.version").to_string()),
        );
    }
    for name in names {
        cmd = cmd.mut_subcommand(name, |sub| localise(sub, false));
    }
    // clap's {all-args} would print English headings, so each section is written out, and only
    // when it has something to show
    let mut template = format!("{{about-with-newline}}\n{} {{usage}}\n", t!("clap.usage"));
    if cmd.get_subcommands().any(|c| !c.is_hide_set()) {
        template += &format!("\n{}\n{{subcommands}}\n", t!("clap.commands"));
    }
    if cmd.get_positionals().any(|a| !a.is_hide_set()) {
        template += &format!("\n{}\n{{positionals}}\n", t!("clap.arguments"));
    }
    template += &format!("\n{}\n{{options}}\n{{after-help}}", t!("clap.options"));
    cmd.help_template(template)
}

/// Prints a command's error. When the server couldn't be reached at all, a translated line says
/// so first; the raw chain ("POST /api/...: io: Connection refused") follows as the detail.
fn report(err: &anyhow::Error) {
    if is_network_error(err) {
        eprintln!("ccn: {}", t!("net.unreachable"));
        eprintln!("     {err:#}");
    } else {
        eprintln!("ccn: {err:#}");
    }
}

/// Whether the error chain holds a transport failure (no connection, DNS, timeout, TLS) rather
/// than an answer from the server.
pub fn is_network_error(err: &anyhow::Error) -> bool {
    use std::io::ErrorKind;
    err.chain().any(|cause| {
        if let Some(e) = cause.downcast_ref::<ureq::Error>() {
            return matches!(
                e,
                ureq::Error::Io(_)
                    | ureq::Error::Timeout(_)
                    | ureq::Error::HostNotFound
                    | ureq::Error::ConnectionFailed
                    | ureq::Error::Tls(_)
                    | ureq::Error::ConnectProxyFailed(_)
                    | ureq::Error::BodyStalled
            );
        }
        cause.downcast_ref::<std::io::Error>().is_some_and(|e| {
            matches!(
                e.kind(),
                ErrorKind::ConnectionRefused
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::NotConnected
                    | ErrorKind::TimedOut
                    | ErrorKind::HostUnreachable
                    | ErrorKind::NetworkUnreachable
                    | ErrorKind::NetworkDown
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::is_network_error;
    use anyhow::Context;

    #[test]
    fn the_localised_command_line_is_valid() {
        // clap's own consistency checks: no clashing -h/-V, every subcommand still reachable
        let mut cmd = super::localise(<super::Cli as clap::CommandFactory>::command(), true);
        cmd.build();
        cmd.clone().debug_assert();
        let help = cmd.render_help().to_string();
        assert!(!help.contains("Print this message"), "{help}");
        assert!(help.contains("--version"), "{help}");
    }

    #[test]
    fn transport_failures_are_network_errors() {
        let refused: anyhow::Result<()> = Err(ureq::Error::Io(std::io::Error::from(
            std::io::ErrorKind::ConnectionRefused,
        )))
        .context("POST /api/pairing/claim-by-client");
        assert!(is_network_error(&refused.unwrap_err()));
        let dns: anyhow::Result<()> = Err(ureq::Error::HostNotFound).context("GET /health");
        assert!(is_network_error(&dns.unwrap_err()));
    }

    #[test]
    fn server_answers_are_not_network_errors() {
        assert!(!is_network_error(&anyhow::anyhow!("server returned 500")));
        let status: anyhow::Result<()> = Err(ureq::Error::StatusCode(502)).context("GET /health");
        assert!(!is_network_error(&status.unwrap_err()));
    }
}
