//! Builds the messages the server expects: the same shapes the Mac app sends on its WebSocket,
//! so the server's push code and the phone apps need no change.

use serde_json::{Value, json};

use crate::session::SessionState;

pub const PERMISSION_OPTIONS: &[(&str, &str)] = &[
    ("Allow once", "y"),
    ("Allow for session", "a"),
    ("Deny", "n"),
];
pub const PLAN_OPTIONS: &[(&str, &str)] = &[
    ("Yes, clear context and auto-accept", "1"),
    ("Yes, and manually approve edits", "2"),
    ("Yes, auto-accept edits", "3"),
    ("Yes, manually approve edits", "4"),
];

pub struct Input<'a>(pub &'a Value);

impl Input<'_> {
    pub fn str(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    }
    pub fn session_id(&self) -> String {
        self.str("session_id").unwrap_or("unknown").to_string()
    }
    pub fn cwd(&self) -> String {
        self.str("cwd").unwrap_or("").to_string()
    }
    pub fn tool_name(&self) -> Option<String> {
        self.str("tool_name").map(|s| s.to_string())
    }
    pub fn tool_input(&self) -> Value {
        self.0.get("tool_input").cloned().unwrap_or(Value::Null)
    }
    pub fn project_name(&self) -> Option<String> {
        let cwd = self.cwd();
        std::path::Path::new(&cwd)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
    }
}

pub fn title(project: Option<&str>) -> String {
    match project {
        Some(p) => format!("Claude Code - {p}"),
        None => "Claude Code".to_string(),
    }
}

fn options(list: &[(&str, &str)]) -> Value {
    Value::Array(
        list.iter()
            .map(|(label, value)| json!({ "label": label, "value": value }))
            .collect(),
    )
}

/// A one-line summary of a tool call, the same way the Mac app shows it.
pub fn tool_summary(tool_name: &str, input: &Value) -> Option<String> {
    let field = |key: &str| {
        input
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    let summary = match tool_name {
        "Read" | "Write" | "Edit" | "NotebookEdit" => {
            field("file_path").or_else(|| field("notebook_path"))
        }
        "Bash" => field("command"),
        "Glob" => field("pattern").map(|p| format!("Pattern: {p}")),
        "Grep" => field("pattern").map(|p| format!("Search: {p}")),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Task" | "Agent" => field("description"),
        "ExitPlanMode" => Some("Approve the plan".to_string()),
        "AskUserQuestion" => first_question(input),
        _ => {
            // First string field, else a compact JSON rendering
            input
                .as_object()
                .and_then(|o| o.values().find_map(|v| v.as_str().map(|s| s.to_string())))
                .or_else(|| {
                    if input.is_null() {
                        None
                    } else {
                        Some(input.to_string())
                    }
                })
        }
    };
    summary.map(|s| SessionState::truncate(&s, 200))
}

pub fn first_question(input: &Value) -> Option<String> {
    input["questions"].as_array()?.first()?["question"]
        .as_str()
        .map(|s| s.to_string())
}

/// Normalises AskUserQuestion input into the question shape the phone renders.
pub fn questions_from_input(input: &Value) -> Vec<Value> {
    let Some(list) = input["questions"].as_array() else {
        return vec![];
    };
    list.iter()
        .filter_map(|q| {
            let question = q["question"].as_str()?;
            let options: Vec<Value> = q["options"]
                .as_array()
                .map(|opts| {
                    opts.iter()
                        .filter_map(|o| {
                            let label = o["label"].as_str()?;
                            Some(json!({ "label": label, "value": label, "description": o["description"].as_str() }))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if options.is_empty() {
                return None;
            }
            Some(json!({
                "question": question,
                "header": q["header"].as_str().unwrap_or("Question"),
                "options": options,
                "multiSelect": q["multiSelect"].as_bool().unwrap_or(false),
            }))
        })
        .collect()
}

fn plan_text(input: &Value) -> Option<String> {
    input["plan"]
        .as_str()
        .or_else(|| input["planContent"].as_str())
        .map(|s| s.to_string())
}

pub fn terminal_context(session_id: &str, cwd: &str, remote: bool) -> Value {
    json!({
        "terminalApp": "ccn",
        "sessionIdentifier": session_id,
        "workingDirectory": cwd,
        "supportsRemoteInput": remote,
    })
}

/// The card for a permission prompt, or a plan approval when the tool is ExitPlanMode.
/// Returns the message, its kind (permission | plan) and a key that identifies the request.
pub fn permission_card(input: &Input, remote: bool) -> (Value, &'static str, String) {
    let tool_name = input.tool_name().unwrap_or_else(|| "Tool".to_string());
    let tool_input = input.tool_input();
    let summary = tool_summary(&tool_name, &tool_input);
    let project = input.project_name();
    let session_id = input.session_id();
    let is_plan = tool_name == "ExitPlanMode";

    let (notification_type, prompt_type, message, prompt_text, opts) = if is_plan {
        let plan = plan_text(&tool_input).unwrap_or_else(|| {
            "Claude has created a plan and is waiting for your approval.".to_string()
        });
        (
            "plan_approval",
            "plan_approval",
            "Plan ready for approval".to_string(),
            plan,
            options(PLAN_OPTIONS),
        )
    } else {
        let description = tool_input["description"].as_str().unwrap_or("");
        let text = match (&summary, description.is_empty()) {
            (Some(s), false) => format!("{tool_name}: {s}\n\n{description}"),
            (Some(s), true) => format!("{tool_name}: {s}"),
            (None, _) => format!("{tool_name} needs permission"),
        };
        (
            "permission_prompt",
            "permission",
            "Permission required".to_string(),
            text,
            options(PERMISSION_OPTIONS),
        )
    };

    let key = format!("{tool_name}:{}", summary.clone().unwrap_or_default());
    let card = json!({
        "type": "notification",
        "sessionId": session_id,
        "notificationType": notification_type,
        "title": title(project.as_deref()),
        "message": message,
        "promptText": prompt_text,
        "promptType": prompt_type,
        "options": opts,
        "terminalContext": terminal_context(&session_id, &input.cwd(), remote),
        "projectName": project,
        "projectPath": input.cwd(),
        "toolName": tool_name,
        "toolInput": summary,
    });
    (card, if is_plan { "plan" } else { "permission" }, key)
}

/// The card for an AskUserQuestion call.
pub fn question_card(input: &Input, questions: &[Value], remote: bool) -> (Value, String) {
    let project = input.project_name();
    let session_id = input.session_id();
    let first = questions
        .first()
        .and_then(|q| q["question"].as_str())
        .unwrap_or("Claude has a question");
    let key = format!("AskUserQuestion:{first}");
    let card = json!({
        "type": "notification",
        "sessionId": session_id,
        "notificationType": "permission_prompt",
        "title": title(project.as_deref()),
        "message": "Claude has questions",
        "promptText": first,
        "promptType": "user_question",
        "questions": questions,
        "terminalContext": terminal_context(&session_id, &input.cwd(), remote),
        "projectName": project,
        "projectPath": input.cwd(),
        "toolName": "AskUserQuestion",
        "toolInput": SessionState::truncate(first, 200),
    });
    (card, key)
}

/// The card for Stop: Claude has finished a turn.
pub fn done_card(input: &Input, state: &SessionState, remote: bool) -> Value {
    let project = input.project_name();
    let session_id = input.session_id();
    let last = input
        .str("last_assistant_message")
        .map(|s| SessionState::truncate(s.trim(), 1500));
    let prompt_text = last
        .filter(|s| !s.is_empty())
        .or_else(|| {
            state
                .prompt
                .as_ref()
                .map(|p| format!("Finished: {}", SessionState::truncate(p, 300)))
        })
        .unwrap_or_else(|| "Claude has finished and is waiting for you.".to_string());
    json!({
        "type": "notification",
        "sessionId": session_id,
        "notificationType": "idle_prompt",
        "title": title(project.as_deref()),
        "message": "Claude has finished",
        "promptText": prompt_text,
        "promptType": "freeform",
        "terminalContext": terminal_context(&session_id, &input.cwd(), remote),
        "projectName": project,
        "projectPath": input.cwd(),
    })
}

pub fn session_update(session_id: &str, status: &str) -> Value {
    json!({ "type": "session_update", "sessionId": session_id, "status": status })
}

pub fn test_notification(device_id: &str) -> Value {
    json!({
        "type": "test_notification",
        "sessionId": format!("test-{}", uuid::Uuid::new_v4()),
        "targetDeviceId": device_id,
    })
}
