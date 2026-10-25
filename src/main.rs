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
mod logger;
mod notify;
mod payload;
mod presence;
mod respond;
mod session;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ccn",
    version,
    about = "Claude Code Notifier: get a push on your phone when Claude Code needs you, and answer from there"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
    /// Remove the hooks, credentials, pairings and local state
    Uninstall,
    /// Run as a Claude Code hook: reads the event JSON on stdin
    #[command(hide = true)]
    Hook,
}

#[derive(Subcommand)]
enum DevicesCommand {
    /// Revoke a paired phone by name or id
    Revoke {
        device: String,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the effective settings
    Show,
    /// Set one setting, e.g. `ccn config set hold_ceiling_seconds 300` or `ccn config set quiet_hours 22:00-08:00`
    Set {
        key: String,
        value: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Hook => hook::run(),
        Command::Pair { code, name, replace_mac_app } => commands::pair(code, name, replace_mac_app),
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
        Command::Uninstall => commands::uninstall(),
    };
    if let Err(err) = result {
        eprintln!("ccn: {err:#}");
        std::process::exit(1);
    }
}
