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

use clap::{Parser, Subcommand};
use rust_i18n::t;

// Every string a person reads lives in locales/<lang>.yml; an unknown language reads English
rust_i18n::i18n!("locales", fallback = "en");

// The help text is read at `Cli::parse()`, after `i18n::init` has chosen the language. clap's own
// scaffolding (Usage, Commands, Options, "Print help") stays English: clap has no translations.
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
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        if let Err(err) = commands::default_command() {
            eprintln!("ccn: {err:#}");
            std::process::exit(1);
        }
        return;
    };
    let result = match command {
        Command::Hook { plugin } => {
            if plugin && hookcfg::settings_has_hooks() {
                return;
            }
            hook::run()
        }
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
    };
    if let Err(err) = result {
        eprintln!("ccn: {err:#}");
        std::process::exit(1);
    }
}
