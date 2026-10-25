//! Turns what the phone sent into what Claude Code's hook contract expects on stdout.

use serde_json::{json, Value};

fn normalise(response: &str) -> String {
    response.trim().to_ascii_lowercase()
}

/// PermissionRequest and plan approval: the phone sends `y` / `a` / `n`, a plan option `1`-`4`,
/// or free text. Free text is a deny with the text as the reason, so Claude reads it.
pub fn permission_output(response: &str) -> Value {
    let decision = match normalise(response).as_str() {
        "y" | "yes" | "allow" | "approve" | "ok" | "a" | "always" | "1" | "2" | "3" | "4" => json!({ "behavior": "allow" }),
        "n" | "no" | "deny" | "reject" => json!({ "behavior": "deny", "message": "Denied from the phone" }),
        _ => json!({ "behavior": "deny", "message": format!("From the phone: {}", response.trim()) }),
    };
    json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } })
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
    updated["questions"] = tool_input.get("questions").cloned().unwrap_or_else(|| json!([]));
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
    let labels: Vec<&str> = question["options"].as_array()?.iter().filter_map(|o| o["label"].as_str()).collect();
    if let Some(rest) = raw.strip_prefix("OTHER:") {
        let text = rest.split_once(':').map(|(_, t)| t).unwrap_or(rest).trim();
        return if text.is_empty() { None } else { Some(text.to_string()) };
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
    if text.is_empty() { None } else { Some(text.to_string()) }
}

/// Stop: free text from the phone becomes Claude's next instruction.
pub fn stop_output(response: &str) -> Option<Value> {
    let text = response.trim();
    if text.is_empty() || matches!(normalise(text).as_str(), "ok" | "done" | "thanks" | "y" | "n") {
        return None;
    }
    Some(json!({ "decision": "block", "reason": text }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions() -> Vec<Value> {
        vec![json!({ "question": "Which framework?", "header": "Framework", "multiSelect": false,
            "options": [{ "label": "React", "value": "React" }, { "label": "Vue", "value": "Vue" }] })]
    }

    #[test]
    fn maps_a_numbered_multi_answer() {
        let out = question_output("[[MULTI_ANSWER]][\"2\"]", &json!({ "questions": [] }), &questions()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["updatedInput"]["answers"]["Which framework?"], "Vue");
    }

    #[test]
    fn maps_other_text() {
        let out = question_output("[[MULTI_ANSWER]][\"OTHER:3:Svelte please\"]", &json!({}), &questions()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["updatedInput"]["answers"]["Which framework?"], "Svelte please");
    }

    #[test]
    fn permission_values() {
        assert_eq!(permission_output("y")["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert_eq!(permission_output("n")["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert_eq!(permission_output("use pnpm instead")["hookSpecificOutput"]["decision"]["message"], "From the phone: use pnpm instead");
    }

    #[test]
    fn stop_block() {
        assert!(stop_output("ok").is_none());
        assert_eq!(stop_output("now write tests").unwrap()["reason"], "now write tests");
    }
}
