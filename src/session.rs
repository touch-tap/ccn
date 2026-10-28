//! Per-session state, one small JSON file per Claude Code session. Written by the fast hooks
//! (PreToolUse, UserPromptSubmit) and read by the ones that notify.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

use crate::config;
use crate::live::Turn;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Terminal {
    pub term_program: Option<String>,
    pub tmux_pane: Option<String>,
    pub term_session_id: Option<String>,
    pub parent_pid: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Card {
    pub notification_id: String,
    /// permission | plan | question | done
    pub kind: String,
    /// What the card was about, to spot a repeat for the same request
    pub key: String,
    pub sent_at: i64,
    /// Whether a hook held for the phone's answer when it sent this card
    pub held: bool,
    /// Whether the server has been told the request is over
    pub settled: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SessionState {
    pub session_id: String,
    pub cwd: String,
    pub started_at: i64,
    pub source: Option<String>,
    pub prompt: Option<String>,
    pub prompt_at: Option<i64>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub tool_at: Option<i64>,
    pub terminal: Terminal,
    /// What the current turn has done, for the phone's lock-screen card
    pub turn: Turn,
    pub last_card: Option<Card>,
    /// Claude Code has shown this session the "not paired any more" message
    pub unpaired_warned: bool,
    pub updated_at: i64,
}

fn path_for(session_id: &str) -> PathBuf {
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    config::sessions_dir().join(format!("{safe}.json"))
}

impl SessionState {
    pub fn load(session_id: &str) -> SessionState {
        let mut state = fs::read_to_string(path_for(session_id))
            .ok()
            .and_then(|text| serde_json::from_str::<SessionState>(&text).ok())
            .unwrap_or_default();
        state.session_id = session_id.to_string();
        state
    }

    pub fn save(&mut self) -> Result<()> {
        self.updated_at = config::now();
        let path = path_for(&self.session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string(self)?)?;
        Ok(())
    }

    pub fn remove(session_id: &str) {
        let _ = fs::remove_file(path_for(session_id));
    }

    /// Records what every hook invocation knows: the working directory and the terminal it runs in.
    pub fn touch(&mut self, input: &Value) {
        if let Some(cwd) = input["cwd"].as_str() {
            self.cwd = cwd.to_string();
        }
        let env = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
        if let Some(v) = env("TERM_PROGRAM") {
            self.terminal.term_program = Some(v);
        }
        if let Some(v) = env("TMUX_PANE") {
            self.terminal.tmux_pane = Some(v);
        }
        if let Some(v) = env("TERM_SESSION_ID").or_else(|| env("ITERM_SESSION_ID")) {
            self.terminal.term_session_id = Some(v);
        }
    }

    pub fn truncate(text: &str, max: usize) -> String {
        if text.chars().count() <= max {
            text.to_string()
        } else {
            let cut: String = text.chars().take(max).collect();
            format!("{cut}...")
        }
    }
}

/// Deletes session files untouched for longer than `days`.
pub fn prune_old(days: i64) {
    let Ok(entries) = fs::read_dir(config::sessions_dir()) else {
        return;
    };
    let cutoff = config::now() - days * 86_400;
    for entry in entries.flatten() {
        let path = entry.path();
        let stale = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<SessionState>(&t).ok())
            .map(|s| s.updated_at < cutoff)
            .unwrap_or(true);
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}
