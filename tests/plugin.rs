//! `ccn hook --plugin`: when the Claude Code plugin's hooks run ccn and when they stand aside.
//! The real binary in a temp home, with no server (a hook that runs is in local mode).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Sandbox {
    root: PathBuf,
}

/// A hooks section with ccn's own entry, as `ccn setup` writes it
const CCN_HOOKS: &str = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/usr/local/bin/ccn hook","timeout":700}]}]}}"#;

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("ccn-plugin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("claude")).unwrap();
        std::fs::create_dir_all(root.join("project/.claude")).unwrap();
        Sandbox { root }
    }

    fn ccn_dir(&self) -> PathBuf {
        self.root.join("ccn")
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    /// What `ccn pair` leaves behind; `ccn uninstall` deletes it
    fn write_config(&self) {
        std::fs::create_dir_all(self.ccn_dir()).unwrap();
        std::fs::write(
            self.ccn_dir().join("config.toml"),
            "desktop_notifications = false\n",
        )
        .unwrap();
    }

    fn write(&self, path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ccn"));
        command
            .args(args)
            .env("HOME", &self.root)
            .env("CCN_HOME", self.ccn_dir())
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("CCN_SERVER_URL", "http://127.0.0.1:9")
            .env("CCN_CREDENTIAL_STORE", "file")
            .env("CCN_NOTIFY", "off")
            .env("CCN_NO_ANALYTICS", "1")
            .env("CCN_NO_AUTO_UPDATE", "1")
            .env("CCN_NO_NUDGE", "1")
            .env("CCN_LANG", "en")
            .env_remove("CLAUDE_PROJECT_DIR");
        command
    }

    /// Runs the plugin's hook for a SessionStart in the project, and says whether ccn handled it
    /// (it records the session when it does)
    fn plugin_ran(&self, project_dir_env: bool) -> bool {
        let session = format!("plugin-test-{}", std::process::id());
        let input = format!(
            r#"{{"hook_event_name":"SessionStart","session_id":"{session}","cwd":"{}","source":"startup"}}"#,
            if project_dir_env {
                "/somewhere/else".to_string()
            } else {
                self.project().join("src").display().to_string()
            }
        );
        let mut command = self.command(&["hook", "--plugin"]);
        if project_dir_env {
            command.env("CLAUDE_PROJECT_DIR", self.project());
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        self.ccn_dir()
            .join("sessions")
            .join(format!("{session}.json"))
            .exists()
    }

    fn run(&self, args: &[&str], dir: &Path) -> Output {
        self.command(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn the_plugin_does_nothing_without_a_config() {
    // Never set up, or uninstalled: the binary is there but ccn has no config
    let sandbox = Sandbox::new("noconfig");
    assert!(!sandbox.plugin_ran(false));
    assert!(!sandbox.ccn_dir().exists(), "nothing was written");
}

#[test]
fn the_plugin_runs_when_no_settings_file_has_the_hooks() {
    let sandbox = Sandbox::new("runs");
    sandbox.write_config();
    assert!(sandbox.plugin_ran(false));
}

#[test]
fn the_plugin_stands_aside_for_the_users_settings() {
    let sandbox = Sandbox::new("user");
    sandbox.write_config();
    sandbox.write(&sandbox.root.join("claude/settings.json"), CCN_HOOKS);
    assert!(!sandbox.plugin_ran(false));
}

#[test]
fn the_plugin_stands_aside_for_the_projects_settings() {
    for name in ["settings.json", "settings.local.json"] {
        let sandbox = Sandbox::new(&name.replace('.', "-"));
        sandbox.write_config();
        sandbox.write(&sandbox.project().join(".claude").join(name), CCN_HOOKS);
        // Found from the project root Claude Code passes, and from a cwd inside the project
        assert!(!sandbox.plugin_ran(true), "{name} via CLAUDE_PROJECT_DIR");
        std::fs::create_dir_all(sandbox.project().join("src/.claude")).unwrap();
        sandbox.write(&sandbox.project().join("src/.claude").join(name), CCN_HOOKS);
        assert!(!sandbox.plugin_ran(false), "{name} via cwd");
    }
}

#[test]
fn doctor_warns_about_hooks_in_more_than_one_settings_file() {
    let sandbox = Sandbox::new("doctor");
    sandbox.write(&sandbox.root.join("claude/settings.json"), CCN_HOOKS);
    let out = sandbox.run(&["doctor"], &sandbox.project());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("more than one settings file"), "{text}");

    sandbox.write(
        &sandbox.project().join(".claude/settings.local.json"),
        CCN_HOOKS,
    );
    let out = sandbox.run(&["doctor"], &sandbox.project());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("more than one settings file"), "{text}");
    assert!(text.contains("settings.local.json"), "{text}");
}
