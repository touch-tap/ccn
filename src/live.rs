//! What the phone's lock-screen card needs to know while Claude works: the current step, and a
//! receipt when it stops. Built from what hooks see, with nothing guessed: a number we cannot
//! derive is left out of the message.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::payload;
use crate::session::SessionState;

/// At most one working update per session in this many seconds
pub const WORKING_EVERY: i64 = 10;

/// What this turn has done so far. Reset when the user sends a prompt.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Turn {
    pub started_at: i64,
    pub working_sent_at: i64,
    pub files: Vec<String>,
    pub approvals: u32,
    pub added: u64,
    pub removed: u64,
    /// A Write replaced a file whose old length is unknown, so added/removed would be a guess
    pub diff_unknown: bool,
    /// The latest step and step count from Claude's todo list
    pub progress: Option<(u32, u32)>,
}

fn lines(text: &str) -> u64 {
    if text.is_empty() {
        0
    } else {
        text.lines().count() as u64
    }
}

impl Turn {
    pub fn begin(now: i64) -> Turn {
        Turn {
            started_at: now,
            ..Turn::default()
        }
    }

    /// Whether a working update is due, and records it when it is
    pub fn working_due(&mut self, now: i64) -> bool {
        if now - self.working_sent_at < WORKING_EVERY {
            return false;
        }
        self.working_sent_at = now;
        true
    }

    pub fn record_approval(&mut self) {
        self.approvals += 1;
    }

    /// Counts the files and lines a tool call is about to change
    pub fn record_tool(&mut self, tool_name: &str, input: &Value) {
        if let Some(progress) = todo_progress(tool_name, input) {
            self.progress = Some(progress);
        }
        let path = input["file_path"]
            .as_str()
            .or_else(|| input["notebook_path"].as_str());
        let edits = matches!(tool_name, "Edit" | "MultiEdit" | "Write" | "NotebookEdit");
        if !edits {
            return;
        }
        if let Some(path) = path.filter(|p| !self.files.iter().any(|f| f == p)) {
            self.files.push(path.to_string());
        }
        match tool_name {
            "Edit" => self.count_edit(input),
            "MultiEdit" => {
                for edit in input["edits"].as_array().into_iter().flatten() {
                    self.count_edit(edit);
                }
            }
            _ => self.diff_unknown = true,
        }
    }

    fn count_edit(&mut self, edit: &Value) {
        self.removed += lines(edit["old_string"].as_str().unwrap_or(""));
        self.added += lines(edit["new_string"].as_str().unwrap_or(""));
    }
}

/// (current step, total steps) from a TodoWrite call: the first item in progress, else the first
/// not yet done. None for any other tool or an empty list.
pub fn todo_progress(tool_name: &str, input: &Value) -> Option<(u32, u32)> {
    if tool_name != "TodoWrite" {
        return None;
    }
    let todos = input["todos"].as_array().filter(|t| !t.is_empty())?;
    let current = todos
        .iter()
        .position(|t| t["status"] == "in_progress")
        .or_else(|| todos.iter().position(|t| t["status"] != "completed"))
        .unwrap_or(todos.len() - 1);
    Some((current as u32 + 1, todos.len() as u32))
}

/// The working update. `detail` is the tool being run, `title` the user's request.
pub fn working(
    session_id: &str,
    project: Option<&str>,
    title: Option<&str>,
    tool: Option<(&str, &Value)>,
    progress: Option<(u32, u32)>,
    started_at: i64,
) -> Value {
    let mut update = json!({ "type": "session_update", "sessionId": session_id, "status": "working", "startedAt": started_at });
    if let Some(p) = project {
        update["projectName"] = json!(p);
    }
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        update["title"] = json!(SessionState::truncate(t.lines().next().unwrap_or(""), 80));
    }
    if let Some((name, input)) = tool {
        let detail = match payload::tool_summary(name, input) {
            Some(summary) => format!("{name}({})", SessionState::truncate(&summary, 60)),
            None => name.to_string(),
        };
        update["detail"] = json!(detail);
    }
    if let Some((step, steps)) = progress {
        update["step"] = json!(step);
        update["steps"] = json!(steps);
    }
    update
}

/// The done receipt. `added` and `removed` are left out when a Write made them unknowable.
pub fn done(
    session_id: &str,
    project: Option<&str>,
    summary: Option<&str>,
    turn: &Turn,
    now: i64,
) -> Value {
    let mut update = json!({
        "type": "session_update", "sessionId": session_id, "status": "done",
        "files": turn.files.len(), "approvals": turn.approvals,
    });
    if let Some(p) = project {
        update["projectName"] = json!(p);
    }
    if let Some(s) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        update["summary"] = json!(SessionState::truncate(s, 200));
    }
    if turn.started_at > 0 && now >= turn.started_at {
        update["durationSeconds"] = json!(now - turn.started_at);
    }
    if !turn.diff_unknown {
        update["added"] = json!(turn.added);
        update["removed"] = json!(turn.removed);
    }
    update
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttles_working_updates() {
        let mut turn = Turn::begin(100);
        assert!(turn.working_due(100));
        assert!(!turn.working_due(105));
        assert!(!turn.working_due(109));
        assert!(turn.working_due(110));
    }

    #[test]
    fn counts_distinct_files_and_edit_lines() {
        let mut turn = Turn::begin(0);
        turn.record_tool(
            "Edit",
            &json!({ "file_path": "a.ts", "old_string": "x\ny", "new_string": "x\ny\nz" }),
        );
        turn.record_tool(
            "Edit",
            &json!({ "file_path": "a.ts", "old_string": "q", "new_string": "r" }),
        );
        turn.record_tool("MultiEdit", &json!({ "file_path": "b.ts", "edits": [
            { "old_string": "", "new_string": "one\ntwo" }, { "old_string": "gone", "new_string": "" }
        ] }));
        turn.record_tool("Read", &json!({ "file_path": "c.ts" }));
        assert_eq!(turn.files, vec!["a.ts", "b.ts"]);
        assert_eq!((turn.added, turn.removed), (6, 4));
        assert!(!turn.diff_unknown);
    }

    #[test]
    fn a_write_makes_the_diff_unknown_and_done_omits_it() {
        let mut turn = Turn::begin(10);
        turn.record_tool("Write", &json!({ "file_path": "n.ts", "content": "a\nb" }));
        turn.record_approval();
        let update = done("s1", Some("proj"), Some("  All good. "), &turn, 70);
        assert_eq!(update["status"], "done");
        assert_eq!(update["files"], 1);
        assert_eq!(update["approvals"], 1);
        assert_eq!(update["durationSeconds"], 60);
        assert_eq!(update["summary"], "All good.");
        assert!(update.get("added").is_none() && update.get("removed").is_none());
    }

    #[test]
    fn done_carries_the_diff_when_known() {
        let mut turn = Turn::begin(0);
        turn.record_tool(
            "Edit",
            &json!({ "file_path": "a", "old_string": "a", "new_string": "b\nc" }),
        );
        let update = done("s1", None, None, &turn, 5);
        assert_eq!(
            (update["added"].as_u64(), update["removed"].as_u64()),
            (Some(2), Some(1))
        );
        assert!(update.get("summary").is_none() && update.get("projectName").is_none());
    }

    #[test]
    fn reads_progress_from_the_todo_list() {
        let todos = json!({ "todos": [
            { "content": "a", "status": "completed" }, { "content": "b", "status": "completed" },
            { "content": "c", "status": "in_progress" }, { "content": "d", "status": "pending" }
        ] });
        assert_eq!(todo_progress("TodoWrite", &todos), Some((3, 4)));
        let none_started = json!({ "todos": [{ "status": "completed" }, { "status": "pending" }] });
        assert_eq!(todo_progress("TodoWrite", &none_started), Some((2, 2)));
        let all_done = json!({ "todos": [{ "status": "completed" }] });
        assert_eq!(todo_progress("TodoWrite", &all_done), Some((1, 1)));
        assert_eq!(todo_progress("Edit", &todos), None);
        assert_eq!(todo_progress("TodoWrite", &json!({ "todos": [] })), None);
    }

    #[test]
    fn remembers_the_latest_todo_progress() {
        let mut turn = Turn::begin(0);
        turn.record_tool(
            "TodoWrite",
            &json!({ "todos": [{ "status": "in_progress" }, { "status": "pending" }] }),
        );
        turn.record_tool("Bash", &json!({ "command": "ls" }));
        assert_eq!(turn.progress, Some((1, 2)));
    }

    #[test]
    fn working_update_shape() {
        let input = json!({ "file_path": "src/auth.ts", "old_string": "a", "new_string": "b" });
        let update = working(
            "s1",
            Some("my-project"),
            Some("Refactor auth\nmore"),
            Some(("Edit", &input)),
            Some((4, 6)),
            100,
        );
        assert_eq!(update["status"], "working");
        assert_eq!(update["title"], "Refactor auth");
        assert_eq!(update["detail"], "Edit(src/auth.ts)");
        assert_eq!(
            (update["step"].as_u64(), update["steps"].as_u64()),
            (Some(4), Some(6))
        );
        assert_eq!(update["projectName"], "my-project");
    }
}
