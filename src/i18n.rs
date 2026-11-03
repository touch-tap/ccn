//! Translations. `rust_i18n::i18n!` in main.rs compiles `locales/<lang>.yml` into the binary;
//! `init` picks the language once, before anything is printed. Every string a person reads goes
//! through `t!("module.key", name = value)` with a literal key that lives in `locales/en.yml`;
//! the tests below keep the Rust and the YAML in step, and every translation in step with English.
//!
//! Not translated, on purpose: log lines, telemetry values, JSON field names, command and setting
//! names, file paths and URLs, the permission option labels sent to the phone (the apps match the
//! English label and show their own translation; payload.rs translates the rest of a card), and
//! the anyhow contexts that wrap an OS error around a path or request ("reading X",
//! "GET url"), which are diagnostics and what `telemetry::failure_reason` classifies.

/// Sets the process locale from `CCN_LANG` or the OS, and makes sure the console can show it.
pub fn init() {
    utf8_console();
    rust_i18n::set_locale(&crate::config::locale());
}

/// Older Windows consoles print bytes in the OEM code page: ask for UTF-8 before the first
/// output. Nothing here can fail the hook; the result is deliberately ignored.
#[cfg(windows)]
fn utf8_console() {
    use windows_sys::Win32::System::Console::SetConsoleOutputCP;
    const CP_UTF8: u32 = 65001;
    // SAFETY: a plain Win32 call with a constant argument and no pointers.
    unsafe {
        SetConsoleOutputCP(CP_UTF8);
    }
}

#[cfg(not(windows))]
fn utf8_console() {}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    const SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    const LOCALES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/locales");

    /// `module.key` → value, flattened the way rust-i18n flattens nested maps.
    fn load(file: &Path) -> BTreeMap<String, String> {
        let text = std::fs::read_to_string(file).unwrap();
        let value: serde_json::Value =
            serde_saphyr::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let mut out = BTreeMap::new();
        flatten("", &value, &mut out);
        out.remove("_version");
        out
    }

    fn flatten(prefix: &str, value: &serde_json::Value, out: &mut BTreeMap<String, String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    flatten(&key, v, out);
                }
            }
            serde_json::Value::String(s) => {
                out.insert(prefix.to_string(), s.clone());
            }
            other => {
                out.insert(prefix.to_string(), other.to_string());
            }
        }
    }

    /// Every literal key passed to `t!(` anywhere under src/. A key that is not a literal would
    /// escape this test, which is why the pattern insists on one.
    fn keys_in_source() -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        for entry in std::fs::read_dir(SRC).unwrap() {
            let path = entry.unwrap().path();
            // This file talks about `t!(` without calling it
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("i18n.rs") {
                continue;
            }
            // Comments may mention t!( too
            let text: String = std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .map(|l| format!("{l}\n"))
                .collect();
            let mut rest = text.as_str();
            while let Some(at) = rest.find("t!(") {
                // `t!(` and not `format!(` or `assert!(`: the byte before must not be an identifier
                let before = rest[..at].chars().next_back();
                rest = &rest[at + 3..];
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let arg = rest.trim_start();
                let Some(literal) = arg.strip_prefix('"') else {
                    panic!(
                        "{}: t!( must take a literal key, found {:?}",
                        path.display(),
                        &arg[..arg.len().min(40)]
                    );
                };
                let end = literal.find('"').unwrap();
                keys.insert(literal[..end].to_string());
            }
        }
        keys
    }

    fn placeholders(value: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = value;
        while let Some(at) = rest.find("%{") {
            rest = &rest[at + 2..];
            let end = rest.find('}').expect("unterminated %{");
            out.insert(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        out
    }

    fn locale_files() -> Vec<std::path::PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(LOCALES)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "yml"))
            .collect();
        files.sort();
        files
    }

    #[test]
    fn every_key_in_the_source_exists_in_english() {
        let en = load(Path::new(LOCALES).join("en.yml").as_path());
        let missing: Vec<_> = keys_in_source()
            .into_iter()
            .filter(|k| !en.contains_key(k))
            .collect();
        assert!(
            missing.is_empty(),
            "t!() keys missing from locales/en.yml: {missing:?}"
        );
    }

    #[test]
    fn every_english_key_is_used_by_the_source() {
        let en = load(Path::new(LOCALES).join("en.yml").as_path());
        let used = keys_in_source();
        let dead: Vec<_> = en.keys().filter(|k| !used.contains(*k)).collect();
        assert!(
            dead.is_empty(),
            "keys in locales/en.yml that no t!() uses: {dead:?}"
        );
    }

    #[test]
    fn translations_have_the_same_keys_and_placeholders_as_english() {
        let en = load(Path::new(LOCALES).join("en.yml").as_path());
        let supported: BTreeSet<&str> = crate::config::LOCALES.iter().copied().collect();
        for file in locale_files() {
            let name = file.file_stem().unwrap().to_str().unwrap();
            assert!(
                supported.contains(name),
                "{name}.yml is not one of config::LOCALES (case matters: pt-BR, zh-Hans)"
            );
            if name == "en" {
                continue;
            }
            let other = load(&file);
            let extra: Vec<_> = other.keys().filter(|k| !en.contains_key(*k)).collect();
            assert!(
                extra.is_empty(),
                "{name}.yml has keys English lacks: {extra:?}"
            );
            let missing: Vec<_> = en.keys().filter(|k| !other.contains_key(*k)).collect();
            assert!(missing.is_empty(), "{name}.yml is missing: {missing:?}");
            for (key, value) in &other {
                assert_eq!(
                    placeholders(value),
                    placeholders(&en[key]),
                    "{name}.yml {key}: placeholders differ from English"
                );
                assert!(!value.is_empty(), "{name}.yml {key} is empty");
            }
        }
    }

    #[test]
    fn an_unknown_locale_falls_back_to_english() {
        // A locale with no file: the English text, never the raw key
        let text = rust_i18n::t!("cli.about", locale = "xx");
        assert!(
            text.starts_with("Claude Code Notifier: get a push"),
            "{text}"
        );
        let text = rust_i18n::t!(
            "pair.paired_with",
            locale = "xx",
            name = "Pixel",
            platform = "android"
        );
        assert_eq!(text, "Paired with Pixel (android).");
    }

    /// Keys whose value may be the same as English in some languages: format strings, command
    /// columns and words such as "file" or "Licence" that the language borrows. Anything else that
    /// matches English word for word was most likely missed by the translator.
    /// A bare `module.key` is allowed in every language; `lang:module.key` in that one only.
    const SAME_AS_ENGLISH_ALLOWED: &[&str] = &[
        // Format only: placeholders, commands and punctuation
        "hook.nudge_message",
        "licence.computer_row",
        "licence.describe_other",
        "licence.replace_example",
        "licence.status_unlicensed",
        "licence.yes_no",
        "licence.no_initial",
        "test.failed",
        // "hooks" is Claude Code's name for the feature, kept in hooks_all of the same files
        "ar:doctor.hooks_error",
        "he:doctor.hooks_error",
        "es:doctor.hooks_error",
        "nl:doctor.hooks_error",
        "pt-BR:doctor.hooks_error",
        // The everyday word in the language is the English one
        "it:doctor.server_error",
        "nl:doctor.server_error",
        "it:doctor.log",
        "nl:doctor.log",
        "pt-BR:doctor.log",
        "it:common.store_file",
        "es:licence.unlocks_no",
        "it:licence.unlocks_no",
        "fr:pair.minutes_one",
        "fr:pair.minutes_other",
        "fr:payload.question_header",
    ];

    #[test]
    fn translations_are_not_left_in_english() {
        let en = load(Path::new(LOCALES).join("en.yml").as_path());
        let allowed: BTreeSet<&str> = SAME_AS_ENGLISH_ALLOWED.iter().copied().collect();
        let mut untranslated = vec![];
        for file in locale_files() {
            let name = file.file_stem().unwrap().to_str().unwrap();
            if name == "en" {
                continue;
            }
            for (key, value) in load(&file) {
                let allowed_here = allowed.contains(key.as_str())
                    || allowed.contains(format!("{name}:{key}").as_str());
                if en.get(&key) == Some(&value) && !allowed_here {
                    untranslated.push(format!("{name}:{key} = {value:?}"));
                }
            }
        }
        assert!(
            untranslated.is_empty(),
            "values identical to English (translate them, or add the key to SAME_AS_ENGLISH_ALLOWED):\n{}",
            untranslated.join("\n")
        );
    }

    #[test]
    fn placeholders_are_found() {
        assert_eq!(
            placeholders("a %{b} c %{d}%{b}"),
            ["b", "d"].into_iter().map(String::from).collect()
        );
        assert!(placeholders("%-d %b %Y").is_empty());
    }
}
