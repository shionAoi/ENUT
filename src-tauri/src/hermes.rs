//! Official `hermes usage --json`; never invoke chat or internal authentication helpers.
use crate::{
    status_command::{self as cmd, CommandRunner, Operation, ProcessRunner, INVALID},
    Observation, Quota,
};
use serde_json::Value;
pub async fn read() -> Result<Observation, String> {
    read_with(&ProcessRunner::default()).await
}
pub async fn read_with(r: &dyn CommandRunner) -> Result<Observation, String> {
    parse(&cmd::json(r, Operation::HermesUsage).await?)
}
pub fn parse(v: &Value) -> Result<Observation, String> {
    if !v["unavailable_reason"].is_null() {
        return Err("Hermes account limits unavailable.".into());
    }
    let mut quotas = vec![];
    for (i, w) in cmd::rows(&v["windows"])?.iter().enumerate() {
        let label = cmd::label(&w["label"])?;
        let remaining = if w["used_percent"].is_null() {
            None
        } else {
            Some(100.0 - cmd::percent(&w["used_percent"])?)
        };
        quotas.push(Quota {
            id: format!("hermes:{i}:{label}"),
            label,
            remaining_percent: remaining,
            resets_at: cmd::reset(&w["resets_at"])?,
            source: "Hermes configured-provider account limits".into(),
        });
    }
    if quotas.is_empty() {
        return Err(INVALID.into());
    }
    Ok(Observation {
        quotas,
        message: Some(
            "Configured-provider limits only; token totals and history unavailable.".into(),
        ),
        ..Default::default()
    })
}
