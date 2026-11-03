//! `ccn forget` end to end: the real binary against a one-request fake server, in a temp home.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Sandbox {
    root: PathBuf,
    /// What `CCN_LANG` says; English unless a test is about the language
    lang: &'static str,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("ccn-forget-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("claude")).unwrap();
        std::fs::create_dir_all(root.join("ccn")).unwrap();
        std::fs::write(
            root.join("ccn/config.toml"),
            "client_id = \"test-computer\"\n",
        )
        .unwrap();
        std::fs::write(root.join("ccn/credentials"), "test-token\n").unwrap();
        std::fs::write(
            root.join("claude/settings.json"),
            r#"{"permissions":{"allow":["Bash(ls)"]}}"#,
        )
        .unwrap();
        Sandbox { root, lang: "en" }
    }

    fn ccn_dir(&self) -> PathBuf {
        self.root.join("ccn")
    }

    fn run(&self, server: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ccn"))
            .args(args)
            .env("HOME", &self.root)
            .env("CCN_HOME", self.ccn_dir())
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("CCN_SERVER_URL", server)
            .env("CCN_CREDENTIAL_STORE", "file")
            .env("CCN_NOTIFY", "off")
            .env("CCN_NO_ANALYTICS", "1")
            .env("CCN_LANG", self.lang)
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

/// Answers one request with `status` and hands back its request line and headers.
fn fake_server(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(reply.as_bytes()).unwrap();
        // Close gracefully: Windows resets a connection closed with unread data, and ccn then
        // sees "connection aborted" instead of the reply
        let _ = stream.shutdown(std::net::Shutdown::Write);
        request
    });
    (url, handle)
}

/// The whole request: headers, then as much body as Content-Length says. Reading all of it before
/// replying matters on Windows, which resets a socket closed with unread bytes.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&data).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                return text;
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return String::from_utf8_lossy(&data).to_string(),
            Ok(n) => data.extend_from_slice(&chunk[..n]),
        }
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn settings_hooks_gone(sandbox: &Sandbox) -> bool {
    let settings =
        std::fs::read_to_string(Path::new(&sandbox.root).join("claude/settings.json")).unwrap();
    !settings.contains("\"hooks\"")
}

#[test]
fn erases_on_the_server_then_removes_everything_local() {
    let sandbox = Sandbox::new("ok");
    let (url, server) = fake_server(200, r#"{"erased":true,"purged":true}"#);
    let out = sandbox.run(&url, &["forget", "--yes"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let request = server.join().unwrap();
    assert!(request.starts_with("DELETE /api/clients/me "), "{request}");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-token"),
        "{request}"
    );
    assert!(
        request.to_ascii_lowercase().contains("accept-language: en"),
        "{request}"
    );
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("Erased this computer and its pairings from the server."),
        "{stdout}"
    );
    assert!(stdout.contains("Erase my licence"), "{stdout}");
    assert!(!sandbox.ccn_dir().exists());
    assert!(settings_hooks_gone(&sandbox));
}

/// The OS language (or `CCN_LANG`) reaches the server as `Accept-Language`, mapped onto one of
/// the supported tags, whatever locales/*.yml files exist.
#[test]
fn asks_the_server_in_the_users_language() {
    let mut sandbox = Sandbox::new("lang");
    sandbox.lang = "pt_PT.UTF-8";
    let (url, server) = fake_server(200, r#"{"erased":true,"purged":true}"#);
    let out = sandbox.run(&url, &["forget", "--yes"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let request = server.join().unwrap();
    assert!(
        request
            .to_ascii_lowercase()
            .contains("accept-language: pt-br"),
        "{request}"
    );
}

#[test]
fn a_401_means_already_gone_so_it_still_cleans_up_locally() {
    let sandbox = Sandbox::new("gone");
    let (url, server) = fake_server(401, r#"{"error":{"message":"unknown"}}"#);
    let out = sandbox.run(&url, &["forget", "-y"]);
    server.join().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("already forgotten"));
    assert!(!sandbox.ccn_dir().exists());
}

#[test]
fn an_unreachable_server_keeps_the_local_credentials() {
    let sandbox = Sandbox::new("offline");
    // A port nothing is listening on
    let url = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let out = sandbox.run(&url, &["forget", "--yes"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("Nothing was removed from this computer"),
        "{}",
        text(&out.stderr)
    );
    assert!(sandbox.ccn_dir().join("credentials").exists());
    assert!(sandbox.ccn_dir().join("config.toml").exists());
}

#[test]
fn a_server_error_keeps_the_local_credentials() {
    let sandbox = Sandbox::new("500");
    let (url, server) = fake_server(500, r#"{"error":{"message":"boom"}}"#);
    let out = sandbox.run(&url, &["forget", "--yes"]);
    server.join().unwrap();
    assert!(!out.status.success());
    assert!(sandbox.ccn_dir().join("credentials").exists());
}

#[test]
fn refuses_without_yes_when_there_is_no_terminal() {
    let sandbox = Sandbox::new("noyes");
    // If this reached the network the connection would fail differently, and the files would still be asserted
    let out = sandbox.run("http://127.0.0.1:9", &["forget"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("--yes"), "{}", text(&out.stderr));
    assert!(sandbox.ccn_dir().join("credentials").exists());
}

fn unreachable_url() -> String {
    // A port nothing is listening on
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

/// `CCN_HOME` may point at a folder that holds other things: only ccn's own files go.
#[test]
fn removes_only_ccns_own_files_from_a_shared_folder() {
    let sandbox = Sandbox::new("shared");
    let dir = sandbox.ccn_dir();
    std::fs::write(dir.join("notes.txt"), "mine").unwrap();
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    std::fs::write(dir.join("sessions/plan.txt"), "mine too").unwrap();
    std::fs::write(
        dir.join("sessions/abc.json"),
        r#"{"session_id":"abc","updated_at":1}"#,
    )
    .unwrap();
    std::fs::write(dir.join("unpaired.json"), "{}").unwrap();
    std::fs::write(dir.join(".config.toml.123-abcdef12.tmp"), "half").unwrap();
    let (url, server) = fake_server(200, r#"{"erased":true,"purged":true}"#);
    let out = sandbox.run(&url, &["forget", "--yes"]);
    server.join().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let mut left: Vec<String> = walk(&dir);
    left.sort();
    assert_eq!(left, vec!["notes.txt", "sessions/plan.txt"]);
    assert!(
        text(&out.stdout).contains("left in place"),
        "{}",
        text(&out.stdout)
    );
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            out.extend(
                walk(&entry.path())
                    .into_iter()
                    .map(|n| format!("{name}/{n}")),
            );
        } else {
            out.push(name);
        }
    }
    out
}

/// Offline, uninstall still removes everything, but says the licence slot may still be held and
/// how to free it, since the token it would need is gone.
#[test]
fn uninstall_offline_says_the_licence_slot_was_not_freed() {
    let sandbox = Sandbox::new("uninstall-offline");
    let out = sandbox.run(&unreachable_url(), &["uninstall"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("Couldn't free this computer's licence slot")
            && stderr.contains("claudecodenotifier.com/licence"),
        "{stderr}"
    );
    assert!(!sandbox.ccn_dir().exists());
    assert!(settings_hooks_gone(&sandbox));
    // The binary is left to whatever installed it, with the command to remove it
    let stdout = text(&out.stdout);
    assert!(stdout.contains("ccn itself is still installed"), "{stdout}");
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_ccn"));
    assert!(exe.exists());
}

/// A server that has already forgotten the computer has no slot for it: nothing to report.
#[test]
fn uninstall_after_the_server_forgot_the_computer_is_quiet_about_the_slot() {
    let sandbox = Sandbox::new("uninstall-gone");
    let (url, server) = fake_server(401, r#"{"error":{"code":"UNAUTHORIZED","message":"no"}}"#);
    let out = sandbox.run(&url, &["uninstall"]);
    let request = server.join().unwrap();
    assert!(
        request.starts_with("POST /api/clients/me/licence/deactivate "),
        "{request}"
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        !text(&out.stderr).contains("licence slot"),
        "{}",
        text(&out.stderr)
    );
    assert!(!sandbox.ccn_dir().exists());
}
