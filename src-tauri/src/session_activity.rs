//! Native running metadata only; never retain directories, names or conversation content.
use crate::{
    status_command::{self as cmd, CommandRunner, Operation, ProcessRunner, INVALID},
    ActiveSession, SessionActivity,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn id(s: &str) -> Result<String, String> {
    if s.is_empty()
        || s.len() > 128
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(INVALID.into());
    }
    Ok(s.into())
}
fn activity(sessions: Vec<ActiveSession>, source: &str, scope: &str) -> SessionActivity {
    SessionActivity {
        sessions,
        source: source.into(),
        scope: scope.into(),
        as_of: crate::dto::now(),
    }
}
pub fn parse_opencode(v: &Value) -> Result<SessionActivity, String> {
    if v.as_object().filter(|m| m.len() == 1).is_none() {
        return Err(INVALID.into());
    }
    let map = v["data"]
        .as_object()
        .filter(|m| m.len() <= 256)
        .ok_or(INVALID)?;
    let mut sessions = Vec::new();
    for (key, row) in map {
        let fields = row.as_object().ok_or(INVALID)?;
        if !key.starts_with("ses") || fields.len() != 1 || row["type"] != "running" {
            return Err(INVALID.into());
        }
        sessions.push(ActiveSession {
            id: id(key)?,
            state: "running".into(),
        });
    }
    Ok(activity(
        sessions,
        "OpenCode GET /api/session/active",
        "Foreground running drains owned by the connected service process only; not all services",
    ))
}
pub async fn read_claude() -> Result<SessionActivity, String> {
    read_claude_with(&ProcessRunner::default()).await
}
pub async fn read_claude_with(r: &dyn CommandRunner) -> Result<SessionActivity, String> {
    parse_claude(&cmd::json(r, Operation::ClaudeAgents).await?)
}
pub fn parse_claude(v: &Value) -> Result<SessionActivity, String> {
    let mut sessions = Vec::new();
    let mut seen = BTreeSet::new();
    for row in cmd::rows(v)? {
        let kind = row["kind"]
            .as_str()
            .filter(|k| ["interactive", "background"].contains(k))
            .ok_or(INVALID)?;
        // Required schema fields are checked but private directories never leave this boundary.
        if row["cwd"].as_str().filter(|s| !s.is_empty()).is_none()
            || row["startedAt"].as_u64().is_none()
        {
            return Err(INVALID.into());
        }
        let state = row
            .get("state")
            .map(|v| {
                v.as_str()
                    .filter(|s| ["working", "blocked", "done", "failed", "stopped"].contains(s))
                    .ok_or(INVALID)
            })
            .transpose()?;
        if kind == "background" && state.is_none() {
            return Err(INVALID.into());
        }
        let status = row
            .get("status")
            .map(|v| {
                v.as_str()
                    .filter(|s| ["busy", "waiting", "idle"].contains(s))
                    .ok_or(INVALID)
            })
            .transpose()?;
        if status.is_some() != row.get("pid").is_some()
            || (status.is_some() && row["pid"].as_u64().filter(|n| *n > 0).is_none())
        {
            return Err(INVALID.into());
        }
        if status == Some("waiting")
            && ![
                "permission prompt",
                "input needed",
                "sandbox request",
                "worker request",
                "dialog open",
            ]
            .contains(&row["waitingFor"].as_str().ok_or(INVALID)?)
        {
            return Err(INVALID.into());
        }
        let key = if kind == "background" {
            row.get("id")
        } else {
            row.get("sessionId")
        };
        let key = key
            .map(|v| id(v.as_str().ok_or(INVALID)?))
            .transpose()?
            .or_else(|| {
                // Claude documents sessionId as optional even for a busy native process.
                if kind == "interactive" && status == Some("busy") {
                    row["pid"].as_u64().map(|pid| format!("process-{pid}"))
                } else {
                    None
                }
            });
        if kind == "background" && key.is_none() {
            return Err(INVALID.into());
        }
        if let Some(key) = &key {
            if !seen.insert(key.clone()) {
                return Err(INVALID.into());
            }
        }
        if status == Some("busy") {
            if kind == "background" && state != Some("working") {
                return Err(INVALID.into());
            }
            sessions.push(ActiveSession {
                id: key.ok_or(INVALID)?,
                state: "running".into(),
            });
        }
    }
    Ok(activity(sessions, "Claude agents --json", "Native live interactive and background sessions for this Claude configuration; busy processes only, not waiting or sleeping work. Native session IDs where provided, otherwise process labels; not stable conversation IDs"))
}
