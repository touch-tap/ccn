//! The hook as Claude Code runs it: a process with JSON on stdin whose output is read to its end.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// A home with nothing in it: not paired, automatic updates on (the default)
    fn new(name: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("ccn-hook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("agent")).unwrap();
        Sandbox { root }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A server that takes one request and does not answer it until told to: whoever asked is alive
/// and waiting for as long as the test likes. Sends the request's path when it arrives.
fn held_server() -> (String, mpsc::Receiver<(String, TcpStream)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sender, requests) = mpsc::channel();
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
            let path = head.split_whitespace().nth(1).unwrap_or_default();
            let _ = sender.send((path.to_string(), stream));
        }
    });
    (url, requests)
}

/// Claude Code reads a hook's output to end-of-file, and a pipe ends only when every process
/// holding its write end has let go. The update check a SessionStart starts outlives the hook (by
/// seconds, or by a whole download), so it must not hold the hook's stdout or stderr: if it did,
/// the session would wait for it (as it did on Windows up to 0.4.5).
#[test]
fn the_hooks_output_closes_as_soon_as_it_has_answered_though_the_update_check_lives_on() {
    let sandbox = Sandbox::new("output-closes");
    let (url, requests) = held_server();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ccn"))
        .arg("hook")
        .env("HOME", &sandbox.root)
        .env("CCN_HOME", sandbox.root.join("home"))
        .env("CLAUDE_CONFIG_DIR", sandbox.root.join("agent"))
        .env("CCN_SERVER_URL", &url)
        .env("CCN_CREDENTIAL_STORE", "file")
        .env("CCN_NOTIFY", "off")
        .env("CCN_PRESENCE", "present")
        .env("CCN_NO_ANALYTICS", "1")
        .env("CCN_NO_NUDGE", "1")
        .env("CCN_LANG", "en")
        .env_remove("CCN_NO_AUTO_UPDATE")
        .env_remove("CCN_INSTALL_METHOD")
        .env_remove("CCN_EPHEMERAL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ =
        child.stdin.take().unwrap().write_all(
            br#"{"hook_event_name":"SessionStart","session_id":"output-closes","cwd":"."}"#,
        );

    // Each stream is read to its end on a thread, so one that stays open fails this test after a
    // few seconds instead of blocking it
    let (sender, ended) = mpsc::channel();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_sender = sender.clone();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = out_sender.send(("stdout", text));
    });
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        let _ = sender.send(("stderr", text));
    });

    // The update check is running, and stays so: its question is not answered yet
    let (path, mut held) = requests
        .recv_timeout(Duration::from_secs(20))
        .expect("the SessionStart hook started an update check, which asked the server");
    assert_eq!(path, "/api/cli/latest");
    assert!(child.wait().unwrap().success());

    let mut open = Vec::new();
    for _ in 0..2 {
        match ended.recv_timeout(Duration::from_secs(5)) {
            Ok((_, text)) => assert_eq!(text.trim(), "", "an unpaired SessionStart says nothing"),
            Err(_) => open.push(()),
        }
    }

    // Nothing newer: the check ends here, without going on to ask GitHub
    let body = format!(
        r#"{{"version":"{VERSION}","tag":"v{VERSION}","assets":["ccn-x86_64-unknown-linux-musl.tar.gz"]}}"#
    );
    let _ = held.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
    // Its end of the connection closes when it has read the answer and gone
    let _ = held.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = held.read(&mut [0u8; 16]);
    assert!(
        open.is_empty(),
        "the hook's output was still open 5 s after it exited: the update check was holding it"
    );
}
