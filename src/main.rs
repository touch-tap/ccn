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

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ccn",
    version,
    about = "Claude Code Notifier: get a push on your phone when Claude Code needs you, and answer from there"
)]
struct Cli {
    /// With no command: pair this computer if it isn't yet, otherwise show its status
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Pair this computer with your phone. Pass the code the app shows, or run with no code to get one to type into the app
    Pair {
        /// The 6-digit code shown in the Claude Code Notifier app
        code: Option<String>,
        /// What this computer is called on your phone (default: the hostname)
        #[arg(long)]
        name: Option<String>,
        /// Also remove the Mac menu bar app's hook entries, so notifications are not sent twice
        #[arg(long)]
        replace_mac_app: bool,
    },
    /// Install the Claude Code hooks (pair does this for you)
    Setup {
        /// Also remove the Mac menu bar app's hook entries
        #[arg(long)]
        replace_mac_app: bool,
    },
    /// Check hooks, credentials, the server and what this terminal supports
    Doctor,
    /// Send a test notification to every paired phone
    Test,
    /// List paired phones, or revoke one
    Devices {
        #[command(subcommand)]
        action: Option<DevicesCommand>,
    },
    /// Show or change this computer's settings (also editable from the phone)
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    /// Pull the latest settings from the server now
    Sync,
    /// Buy a licence for this computer in the browser: unlocks every phone paired with it
    Upgrade {
        /// Pay once instead of yearly (or switch an annual licence to lifetime, keeping the key)
        #[arg(long)]
        lifetime: bool,
    },
    /// Use a licence key on this computer (up to 3 computers per licence)
    Activate {
        /// The key from your email, CCN-XXXX-XXXX-XXXX-XXXX
        key: String,
        /// Free the slot of another computer on the licence, by its name, to make room for this one
        #[arg(long)]
        replace: Option<String>,
    },
    /// Free this computer's licence slot so the key can be used elsewhere
    Deactivate,
    /// Show this computer's licence and the computers using it
    #[command(alias = "license")]
    Licence,
    /// Remove the hooks, credentials, pairings and local state
    Uninstall,
    /// Erase this computer and its pairings from the server, then uninstall
    Forget {
        /// Don't ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Run as a Claude Code hook: reads the event JSON on stdin
    #[command(hide = true)]
    Hook {
        /// Called by the Claude Code plugin, which stands aside when settings.json has ccn's hooks too
        #[arg(long)]
        plugin: bool,
    },
}

#[derive(Subcommand)]
enum DevicesCommand {
    /// Revoke a paired phone by name or id
    Revoke { device: String },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the effective settings
    Show,
    /// Set one setting, e.g. `ccn config set hold_ceiling_seconds 300` or `ccn config set quiet_hours 22:00-08:00`
    Set { key: String, value: String },
}

fn main() {
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
        } => commands::pair(code, name, replace_mac_app),
        Command::Setup { replace_mac_app } => commands::setup(replace_mac_app),
        Command::Doctor => commands::doctor(),
        Command::Test => commands::test(),
        Command::Devices { action } => match action {
            None => commands::devices(),
            Some(DevicesCommand::Revoke { device }) => commands::revoke(&device),
        },
        Command::Config { action } => match action {
            ConfigCommand::Show => commands::config_show(),
            ConfigCommand::Set { key, value } => commands::config_set(&key, &value),
        },
        Command::Sync => commands::sync(),
        Command::Upgrade { lifetime } => licence::upgrade(lifetime),
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
