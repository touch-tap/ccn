//! `ccn upgrade` end to end: the real binary, copied into a temp bin directory, against a fake
//! server that serves `/api/cli/latest`, a release archive holding a tiny script called `ccn`,
//! and its SHA256SUMS.

#![cfg(unix)] // the release archive is a tar.gz of a shell script; Windows gets a zip of an exe

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NEW_VERSION: &str = "9.9.9";
const SCRIPT: &str = "#!/bin/sh\necho \"ccn 9.9.9\"\n";

fn asset_name() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => format!("ccn-{arch}-apple-darwin.tar.gz"),
        _ => format!("ccn-{arch}-unknown-linux-musl.tar.gz"),
    }
}

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// A home with ccn's config, Claude's settings (hooks pointing at the sandbox binary) and a
    /// copy of the real ccn at `bin/ccn`, or at `exe_rel` when given.
    fn new(name: &str, exe_rel: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("ccn-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("claude")).unwrap();
        std::fs::create_dir_all(root.join("ccn")).unwrap();
        let exe = root.join(exe_rel);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_ccn"), &exe).unwrap();
        // As `ccn setup` writes it: the canonical path (macOS puts temp dirs under /private)
        let exe = exe.canonicalize().unwrap();
        std::fs::write(root.join("ccn/config.toml"), "auto_update = true\n").unwrap();
        let hook = format!("{} hook", exe.display());
        std::fs::write(
            root.join("claude/settings.json"),
            format!(
                r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"{hook}","timeout":700}}]}}]}}}}"#
            ),
        )
        .unwrap();
        Sandbox { root }
    }

    fn exe(&self) -> PathBuf {
        self.root.join("bin/ccn").canonicalize().unwrap()
    }

    fn run_exe(&self, exe: &PathBuf, server: &str, args: &[&str]) -> Output {
        Command::new(exe)
            .args(args)
            .env("HOME", &self.root)
            .env("CCN_HOME", self.root.join("ccn"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("CCN_SERVER_URL", server)
            .env("CCN_CREDENTIAL_STORE", "file")
            .env("CCN_NOTIFY", "off")
            .env("CCN_NO_ANALYTICS", "1")
            .env_remove("CCN_INSTALL_METHOD")
            .env_remove("CCN_EPHEMERAL")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn run(&self, server: &str, args: &[&str]) -> Output {
        self.run_exe(&self.exe(), server, args)
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.root.join("ccn/config.toml")).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A release: the archive holding the script, and the SHA256SUMS line for it.
fn release(dir: &PathBuf, correct_checksum: bool) -> (Vec<u8>, String) {
    let stage = dir.join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::write(stage.join("ccn"), SCRIPT).unwrap();
    let archive = dir.join(asset_name());
    let status = Command::new("tar")
        .arg("czf")
        .arg(&archive)
        .arg("-C")
        .arg(&stage)
        .arg("ccn")
        .status()
        .unwrap();
    assert!(status.success(), "tar failed");
    let bytes = std::fs::read(&archive).unwrap();
    let hash = if correct_checksum {
        use sha2::Digest;
        sha2::Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    } else {
        "0".repeat(64)
    };
    (bytes, format!("{hash}  {}\n", asset_name()))
}

type Routes = HashMap<String, (&'static str, Vec<u8>)>;

/// Serves `routes` by path for as long as the test runs, remembering what was asked for.
fn serve(build: impl FnOnce(&str) -> Routes) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let routes = build(&url);
    let requested = Arc::new(Mutex::new(Vec::new()));
    let seen = requested.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let head = String::from_utf8_lossy(&request).to_string();
            let path = head
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            seen.lock().unwrap().push(path.clone());
            let reply = match routes.get(&path) {
                Some((ctype, body)) => {
                    let mut r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    r.extend_from_slice(body);
                    r
                }
                None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            };
            let _ = stream.write_all(&reply);
        }
    });
    (url, requested)
}

/// A server whose latest release is `version`, with the archive and checksums under `/dl`.
fn release_server(
    dir: &PathBuf,
    version: &str,
    correct_checksum: bool,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let (archive, sums) = release(dir, correct_checksum);
    let version = version.to_string();
    serve(move |url| {
        let latest = format!(
            r#"{{"version":"{version}","tag":"v{version}","publishedAt":1761800000,"downloadBase":"{url}/dl","assets":["{}","SHA256SUMS"]}}"#,
            asset_name()
        );
        HashMap::from([
            (
                "/api/cli/latest".to_string(),
                ("application/json", latest.into_bytes()),
            ),
            (
                format!("/dl/{}", asset_name()),
                ("application/gzip", archive),
            ),
            (
                "/dl/SHA256SUMS".to_string(),
                ("text/plain", sums.into_bytes()),
            ),
        ])
    })
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[test]
fn replaces_the_binary_with_the_verified_release() {
    let sandbox = Sandbox::new("ok", "bin/ccn");
    let original = std::fs::read(sandbox.exe()).unwrap();
    let (url, requested) = release_server(&sandbox.root, NEW_VERSION, true);

    let out = sandbox.run(&url, &["upgrade"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        format!("Updated ccn {VERSION} → {NEW_VERSION}.")
    );

    let requested = requested.lock().unwrap().clone();
    assert_eq!(
        requested,
        vec![
            "/api/cli/latest".to_string(),
            format!("/dl/{}", asset_name()),
            "/dl/SHA256SUMS".to_string(),
        ]
    );

    // The release's ccn is now the installed one, the old binary is gone, nothing is left beside it
    let installed = std::fs::read(sandbox.exe()).unwrap();
    assert_eq!(text(&installed), SCRIPT);
    assert_ne!(installed, original);
    assert!(!sandbox.root.join("bin/ccn.new").exists());
    assert!(!sandbox.root.join("bin/ccn.old").exists());
    assert_eq!(
        std::fs::read_dir(sandbox.root.join("bin")).unwrap().count(),
        1
    );
    let version = Command::new(sandbox.exe())
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(text(&version.stdout).trim(), "ccn 9.9.9");

    // The hooks already pointed at this path, so settings.json was not rewritten
    let settings = std::fs::read_to_string(sandbox.root.join("claude/settings.json")).unwrap();
    assert!(settings.contains(&format!("{} hook", sandbox.exe().display())));
    assert!(
        !sandbox
            .root
            .join("claude/settings.json.ccn-backup")
            .exists()
    );
}

#[test]
fn a_bad_checksum_leaves_the_original_untouched() {
    let sandbox = Sandbox::new("badsum", "bin/ccn");
    let original = std::fs::read(sandbox.exe()).unwrap();
    let (url, _) = release_server(&sandbox.root, NEW_VERSION, false);

    let out = sandbox.run(&url, &["upgrade"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("checksum mismatch"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(std::fs::read(sandbox.exe()).unwrap(), original);
    assert!(!sandbox.root.join("bin/ccn.new").exists());
    let still_runs = sandbox.run(&url, &["--version"]);
    assert_eq!(text(&still_runs.stdout).trim(), format!("ccn {VERSION}"));
}

#[test]
fn says_so_when_already_the_latest() {
    let sandbox = Sandbox::new("latest", "bin/ccn");
    let original = std::fs::read(sandbox.exe()).unwrap();
    let (url, requested) = release_server(&sandbox.root, VERSION, true);

    let out = sandbox.run(&url, &["upgrade"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        format!("ccn {VERSION} is the latest.")
    );
    assert_eq!(std::fs::read(sandbox.exe()).unwrap(), original);
    assert_eq!(requested.lock().unwrap().len(), 1, "nothing was downloaded");

    let quiet = sandbox.run(&url, &["update", "--quiet"]);
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty(), "{}", text(&quiet.stdout));
}

#[test]
fn check_only_reports_and_exits_10_when_newer() {
    let sandbox = Sandbox::new("check", "bin/ccn");
    let original = std::fs::read(sandbox.exe()).unwrap();
    let (url, requested) = release_server(&sandbox.root, NEW_VERSION, true);

    let out = sandbox.run(&url, &["upgrade", "--check"]);
    assert_eq!(out.status.code(), Some(10), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains(&format!(
            "ccn {NEW_VERSION} is available (you have {VERSION})"
        )),
        "{stdout}"
    );
    assert!(stdout.contains("Update: ccn upgrade"), "{stdout}");
    assert_eq!(std::fs::read(sandbox.exe()).unwrap(), original);
    assert_eq!(requested.lock().unwrap().len(), 1, "nothing was downloaded");

    let (url, _) = release_server(&sandbox.root, VERSION, true);
    let out = sandbox.run(&url, &["upgrade", "--check"]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn a_homebrew_install_is_told_to_brew_upgrade_and_the_background_run_records_it() {
    let sandbox = Sandbox::new("brew", "homebrew/Cellar/ccn/0.1.0/bin/ccn");
    let exe = sandbox.root.join("homebrew/Cellar/ccn/0.1.0/bin/ccn");
    let original = std::fs::read(&exe).unwrap();
    let (url, requested) = release_server(&sandbox.root, NEW_VERSION, true);

    let out = sandbox.run_exe(&exe, &url, &["upgrade"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        format!("ccn {NEW_VERSION} is available: brew upgrade touch-tap/tap/ccn")
    );
    assert_eq!(std::fs::read(&exe).unwrap(), original);
    assert!(!sandbox.config().contains("update_available"));

    let out = sandbox.run_exe(&exe, &url, &["upgrade", "--quiet", "--background"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    assert!(
        sandbox
            .config()
            .contains(&format!("update_available = \"{NEW_VERSION}\"")),
        "{}",
        sandbox.config()
    );
    assert_eq!(std::fs::read(&exe).unwrap(), original);
    assert_eq!(
        requested.lock().unwrap().len(),
        2,
        "only the version was asked for"
    );

    // Once brew has done it, the next background run clears the note
    let (url, _) = release_server(&sandbox.root, VERSION, true);
    let out = sandbox.run_exe(&exe, &url, &["upgrade", "--quiet", "--background"]);
    assert!(out.status.success());
    assert!(!sandbox.config().contains("update_available"));
}

#[test]
fn auto_update_is_a_local_setting() {
    let sandbox = Sandbox::new("setting", "bin/ccn");
    let out = sandbox.run(
        "http://127.0.0.1:9",
        &["config", "set", "auto_update", "off"],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("auto_update                off"));
    assert!(sandbox.config().contains("auto_update = false"));
    let out = sandbox.run(
        "http://127.0.0.1:9",
        &["config", "set", "auto_update", "maybe"],
    );
    assert!(!out.status.success());
}
