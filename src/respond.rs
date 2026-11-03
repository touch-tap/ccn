//! Turns what the phone sent into what Claude Code's hook contract expects on stdout.

use serde_json::{Value, json};

fn normalise(response: &str) -> String {
    response.trim().to_ascii_lowercase()
}

/// PermissionRequest and plan approval. `request` is the hook's input, for the tool and Claude
/// Code's own suggestions. The phone sends `y` / `a` / `n`, a plan option (`3` auto-accept edits,
/// `4` approve each edit; `1` and `2` from cards sent before Nov 2026 offered to clear context as
/// well, which a hook can't do, so they mean the same without it), or free text. Free text is a
/// deny with the text as the reason, so Claude reads it.
pub fn permission_output(response: &str, request: &Value) -> Value {
    let tool_name = request["tool_name"].as_str().unwrap_or("");
    let is_plan = tool_name == "ExitPlanMode";
    let allow = |updates: Vec<Value>| {
        if updates.is_empty() {
            json!({ "behavior": "allow" })
        } else {
            json!({ "behavior": "allow", "updatedPermissions": updates })
        }
    };
    let decision = match (normalise(response).as_str(), is_plan) {
        // Approving a plan from a hook leaves the session in the mode it had before plan mode,
        // unless the answer names one
        ("1" | "3" | "a" | "always", true) => allow(vec![set_mode("acceptEdits")]),
        ("2" | "4", true) => allow(vec![set_mode("default")]),
        ("a" | "always", false) => allow(session_permissions(request)),
        ("y" | "yes" | "allow" | "approve" | "ok" | "1" | "2" | "3" | "4", _) => allow(vec![]),
        ("n" | "no" | "deny" | "reject", true) => json!({
            "behavior": "deny",
            "message": "Not approved from the phone: keep planning, and don't make changes yet"
        }),
        ("n" | "no" | "deny" | "reject", false) => {
            json!({ "behavior": "deny", "message": "Denied from the phone" })
        }
        _ => {
            json!({ "behavior": "deny", "message": format!("From the phone: {}", response.trim()) })
        }
    };
    json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } })
}

fn set_mode(mode: &str) -> Value {
    json!({ "type": "setMode", "mode": mode, "destination": "session" })
}

/// What "Allow for session" adds: the rules Claude Code itself suggested for this request (its
/// "don't ask again" options), kept to this session, since the phone's button says so. Without
/// suggestions (an older Claude Code), a rule built the same way: edits switch the session to
/// accepting edits, a command is allowed as a prefix where that is safe to tell, and a web fetch
/// for its domain.
fn session_permissions(request: &Value) -> Vec<Value> {
    if let Some(suggestions) = request["permission_suggestions"].as_array() {
        let scoped: Vec<Value> = suggestions
            .iter()
            .filter(|s| s.is_object() && s["type"].is_string())
            .map(|s| {
                let mut s = s.clone();
                s["destination"] = json!("session");
                s
            })
            .collect();
        if !scoped.is_empty() {
            return scoped;
        }
    }
    let tool_name = request["tool_name"].as_str().unwrap_or("");
    let input = &request["tool_input"];
    let rule = |content: Option<String>| {
        let mut rule = json!({ "toolName": tool_name });
        if let Some(content) = content {
            rule["ruleContent"] = json!(content);
        }
        json!({ "type": "addRules", "rules": [rule], "behavior": "allow", "destination": "session" })
    };
    match tool_name {
        "" => vec![],
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => vec![set_mode("acceptEdits")],
        "Bash" => match input["command"].as_str().map(str::trim) {
            Some(command) if !command.is_empty() => vec![rule(Some(bash_rule(command)))],
            _ => vec![],
        },
        "WebFetch" => match input["url"].as_str().and_then(url_host) {
            Some(host) => vec![rule(Some(format!("domain:{host}")))],
            None => vec![],
        },
        _ => vec![rule(None)],
    }
}

/// Programs whose first argument picks what they do, so `git status` is a family worth allowing
/// as a whole and `git` alone is not
const SUBCOMMAND_TOOLS: &[&str] = &[
    "git",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "npx",
    "cargo",
    "go",
    "docker",
    "kubectl",
    "make",
    "swift",
    "xcodebuild",
    "gradle",
    "./gradlew",
    "pip",
    "pip3",
    "uv",
    "poetry",
    "brew",
    "gh",
    "dotnet",
    "mvn",
    "deno",
    "rustup",
    "terraform",
];

/// A Bash rule for "allow for session". A simple `git status --short` becomes the prefix rule
/// `git status *`, which matches the bare command too, the form Claude Code's own "don't ask
/// again" writes. Anything else is allowed exactly as written: a prefix of `rm -rf build` (`rm *`)
/// or of a pipeline would allow far more than the user saw.
fn bash_rule(command: &str) -> String {
    let compound = command.chars().any(|c| {
        matches!(
            c,
            ';' | '&' | '|' | '<' | '>' | '`' | '$' | '\n' | '(' | ')'
        )
    });
    let mut words = command.split_whitespace();
    if let (false, Some(program), Some(sub)) = (compound, words.next(), words.next()) {
        let plain = sub
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'));
        if SUBCOMMAND_TOOLS.contains(&program) && plain && !sub.starts_with('-') {
            // `npm run` and the like run anything; the script they name is the family
            if matches!(sub, "run" | "exec" | "x" | "run-script") {
                return match words.next() {
                    Some(script) if !script.starts_with('-') => {
                        format!("{program} {sub} {script} *")
                    }
                    _ => command.to_string(),
                };
            }
            return format!("{program} {sub} *");
        }
    }
    command.to_string()
}

/// `https://docs.rs/serde/latest` → `docs.rs`
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// AskUserQuestion: the phone sends either one option label, or `[[MULTI_ANSWER]]` followed by a
/// JSON array with one entry per question: a 1-based option number, or `OTHER:N:text`.
/// Returns None when nothing usable could be mapped, so the terminal asks as normal.
pub fn question_output(response: &str, tool_input: &Value, questions: &[Value]) -> Option<Value> {
    let mut answers = serde_json::Map::new();
    let trimmed = response.trim();

    if let Some(rest) = trimmed.strip_prefix("[[MULTI_ANSWER]]") {
        let list: Vec<String> = serde_json::from_str(rest).ok()?;
        for (i, raw) in list.iter().enumerate() {
            let Some(q) = questions.get(i) else { break };
            let question = q["question"].as_str()?.to_string();
            let answer = map_answer(raw, q)?;
            answers.insert(question, json!(answer));
        }
    } else {
        let q = questions.first()?;
        let question = q["question"].as_str()?.to_string();
        let answer = map_answer(trimmed, q)?;
        answers.insert(question, json!(answer));
    }
    if answers.is_empty() {
        return None;
    }

    let mut updated = tool_input.clone();
    if !updated.is_object() {
        updated = json!({});
    }
    updated["questions"] = tool_input
        .get("questions")
        .cloned()
        .unwrap_or_else(|| json!([]));
    updated["answers"] = Value::Object(answers);
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": updated,
        }
    }))
}

fn map_answer(raw: &str, question: &Value) -> Option<String> {
    let labels: Vec<&str> = question["options"]
        .as_array()?
        .iter()
        .filter_map(|o| o["label"].as_str())
        .collect();
    if let Some(rest) = raw.strip_prefix("OTHER:") {
        let text = rest.split_once(':').map(|(_, t)| t).unwrap_or(rest).trim();
        return if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        };
    }
    if let Ok(n) = raw.trim().parse::<usize>() {
        if n >= 1 {
            if let Some(label) = labels.get(n - 1) {
                return Some(label.to_string());
            }
        }
    }
    if let Some(label) = labels.iter().find(|l| l.eq_ignore_ascii_case(raw.trim())) {
        return Some(label.to_string());
    }
    // Free text typed on the phone becomes the answer as written
    let text = raw.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Stop: free text from the phone becomes Claude's next instruction.
pub fn stop_output(response: &str) -> Option<Value> {
    let text = response.trim();
    if text.is_empty()
        || matches!(
            normalise(text).as_str(),
            "ok" | "done" | "thanks" | "y" | "n"
        )
    {
        return None;
    }
    Some(json!({ "decision": "block", "reason": text }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions() -> Vec<Value> {
        vec![
            json!({ "question": "Which framework?", "header": "Framework", "multiSelect": false,
            "options": [{ "label": "React", "value": "React" }, { "label": "Vue", "value": "Vue" }] }),
        ]
    }

    #[test]
    fn maps_a_numbered_multi_answer() {
        let out = question_output(
            "[[MULTI_ANSWER]][\"2\"]",
            &json!({ "questions": [] }),
            &questions(),
        )
        .unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["answers"]["Which framework?"],
            "Vue"
        );
    }

    #[test]
    fn maps_other_text() {
        let out = question_output(
            "[[MULTI_ANSWER]][\"OTHER:3:Svelte please\"]",
            &json!({}),
            &questions(),
        )
        .unwrap();
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["answers"]["Which framework?"],
            "Svelte please"
        );
    }

    fn decision(response: &str, request: &Value) -> Value {
        permission_output(response, request)["hookSpecificOutput"]["decision"].clone()
    }

    #[test]
    fn permission_values() {
        let bash = json!({ "tool_name": "Bash", "tool_input": { "command": "ls" } });
        assert_eq!(decision("y", &bash), json!({ "behavior": "allow" }));
        assert_eq!(decision("n", &bash)["behavior"], "deny");
        assert_eq!(
            decision("use pnpm instead", &bash)["message"],
            "From the phone: use pnpm instead"
        );
        assert_eq!(
            permission_output("y", &bash)["hookSpecificOutput"]["hookEventName"],
            "PermissionRequest"
        );
    }

    #[test]
    fn allow_for_session_keeps_claude_codes_suggestions_to_the_session() {
        let request = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf node_modules" },
            "permission_suggestions": [{
                "type": "addRules",
                "rules": [{ "toolName": "Bash", "ruleContent": "rm -rf node_modules" }],
                "behavior": "allow",
                "destination": "localSettings"
            }]
        });
        assert_eq!(
            decision("a", &request),
            json!({ "behavior": "allow", "updatedPermissions": [{
                "type": "addRules",
                "rules": [{ "toolName": "Bash", "ruleContent": "rm -rf node_modules" }],
                "behavior": "allow",
                "destination": "session"
            }] })
        );
    }

    fn session_rule(tool: &str, input: Value) -> Value {
        decision("a", &json!({ "tool_name": tool, "tool_input": input }))["updatedPermissions"]
            .clone()
    }

    #[test]
    fn allow_for_session_without_suggestions() {
        let rule = |tool: &str, content: Option<&str>| {
            let mut r = json!({ "toolName": tool });
            if let Some(c) = content {
                r["ruleContent"] = json!(c);
            }
            json!([{ "type": "addRules", "rules": [r], "behavior": "allow", "destination": "session" }])
        };
        let accept_edits =
            json!([{ "type": "setMode", "mode": "acceptEdits", "destination": "session" }]);
        // A subcommand family becomes a prefix rule
        assert_eq!(
            session_rule("Bash", json!({ "command": "git status --short" })),
            rule("Bash", Some("git status *"))
        );
        assert_eq!(
            session_rule("Bash", json!({ "command": "npm run test:unit" })),
            rule("Bash", Some("npm run test:unit *"))
        );
        assert_eq!(
            session_rule("Bash", json!({ "command": "cargo build --release" })),
            rule("Bash", Some("cargo build *"))
        );
        // Anything that a prefix would widen too far stays exact
        for exact in [
            "rm -rf build",
            "git -C /tmp log",
            "git log | head",
            "cargo test && rm x",
            "echo $(whoami)",
            "ls",
            "npm run",
        ] {
            assert_eq!(
                session_rule("Bash", json!({ "command": exact })),
                rule("Bash", Some(exact)),
                "{exact}"
            );
        }
        assert_eq!(
            session_rule("Edit", json!({ "file_path": "/a" })),
            accept_edits
        );
        assert_eq!(
            session_rule("Write", json!({ "file_path": "/a" })),
            accept_edits
        );
        assert_eq!(
            session_rule(
                "WebFetch",
                json!({ "url": "https://Docs.rs:443/serde?x=1" })
            ),
            rule("WebFetch", Some("domain:docs.rs"))
        );
        assert_eq!(
            session_rule("mcp__github__create_issue", json!({ "title": "x" })),
            rule("mcp__github__create_issue", None)
        );
        // Nothing to build a rule from: a plain allow
        assert_eq!(
            decision("a", &json!({ "tool_name": "Bash", "tool_input": {} })),
            json!({ "behavior": "allow" })
        );
    }

    #[test]
    fn plan_answers() {
        let plan = json!({ "tool_name": "ExitPlanMode", "tool_input": { "plan": "Do it" } });
        let mode = |m: &str| {
            json!({ "behavior": "allow", "updatedPermissions": [
                { "type": "setMode", "mode": m, "destination": "session" }
            ] })
        };
        // The options sent now
        assert_eq!(crate::payload::PLAN_VALUES, ["3", "4", "n"]);
        assert_eq!(decision("3", &plan), mode("acceptEdits"));
        assert_eq!(decision("4", &plan), mode("default"));
        let keep = decision("n", &plan);
        assert_eq!(keep["behavior"], "deny");
        assert!(keep["message"].as_str().unwrap().contains("keep planning"));
        // Cards from older versions offered clearing context, which a hook can't do
        assert_eq!(decision("1", &plan), mode("acceptEdits"));
        assert_eq!(decision("2", &plan), mode("default"));
        // Plain approval keeps the mode Claude Code restores; feedback keeps planning
        assert_eq!(decision("y", &plan), json!({ "behavior": "allow" }));
        assert_eq!(
            decision("split step 2", &plan)["message"],
            "From the phone: split step 2"
        );
    }

    #[test]
    fn stop_block() {
        assert!(stop_output("ok").is_none());
        assert_eq!(
            stop_output("now write tests").unwrap()["reason"],
            "now write tests"
        );
    }
}
