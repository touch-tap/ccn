// Embeds the macOS notifier app (scripts/build-macos-notifier.sh) in the ccn binary when
// CCN_NOTIFIER_APP points at a built "Claude Code Notifier.app". Without it, ccn falls back to
// osascript notifications. Generates notifier_files.rs: (path inside the bundle, bytes, executable).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=CCN_NOTIFIER_APP");
    let mut out = String::from("pub const FILES: &[(&str, &[u8], bool)] = &[\n");

    let app = std::env::var("CCN_NOTIFIER_APP")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if let (Some(app), true) = (app, macos) {
        let app = app
            .canonicalize()
            .expect("CCN_NOTIFIER_APP must point at a built app bundle");
        println!("cargo:rerun-if-changed={}", app.display());
        let mut files = Vec::new();
        walk(&app, &mut files);
        files.sort();
        for file in files {
            let rel = file
                .strip_prefix(&app)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let executable = rel.starts_with("Contents/MacOS/");
            writeln!(
                out,
                "    ({rel:?}, include_bytes!({:?}), {executable}),",
                file.display().to_string()
            )
            .unwrap();
        }
    }

    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("notifier_files.rs");
    std::fs::write(dest, out).unwrap();
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}
