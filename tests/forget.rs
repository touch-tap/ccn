//! `ccn forget` end to end: the real binary against a one-request fake server, in a temp home.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Sandbox {
    root: PathBuf,
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
        Sandbox { root }
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
            .env("CCN_LANG", "pt_PT.UTF-8")
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
        let mut request = vec![0u8; 4096];
        let n = stream.read(&mut request).unwrap();
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(reply.as_bytes()).unwrap();
        String::from_utf8_lossy(&request[..n]).to_string()
    });
    (url, handle)
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
        request
            .to_ascii_lowercase()
            .contains("accept-language: pt-br"),
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
