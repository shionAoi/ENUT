//! OpenCode V2 GET-only API adapter. Never read credentials or session content.
use crate::{
    status_command::{self as cmd, CommandRunner, Operation, ProcessRunner, INVALID},
    ContextInfo, McpInfo, ModelUsage, Observation, Quota, Usage,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
pub async fn read() -> Result<Observation, String> {
    read_with(&ProcessRunner::default()).await
}
pub async fn read_with(r: &dyn CommandRunner) -> Result<Observation, String> {
    let mut out = Observation::default();
    let mut successes = 0;
    let mut missing = false;
    match cmd::json(r, Operation::OpenCodeStats)
        .await
        .and_then(|v| parse_stats(&v))
    {
        Ok(o) => {
            out = o;
            successes += 1;
        }
        Err(_) => missing = true,
    }
    match cmd::json(r, Operation::OpenCodeLimits)
        .await
        .and_then(|v| parse_limits(&v))
    {
        Ok(q) => {
            out.quotas = q;
            successes += 1;
        }
        Err(_) => missing = true,
    }
    let mut context = ContextInfo::default();
    match cmd::json(r, Operation::OpenCodeMcps)
        .await
        .and_then(|v| parse_context(Some(&v), None))
    {
        Ok(c) => {
            context.mcps = c.mcps;
            successes += 1;
        }
        Err(_) => missing = true,
    }
    match cmd::json(r, Operation::OpenCodeSkills)
        .await
        .and_then(|v| parse_context(None, Some(&v)))
    {
        Ok(c) => {
            context.skills = c.skills;
            successes += 1;
        }
        Err(_) => missing = true,
    }
    match cmd::json(r, Operation::OpenCodeActiveSessions)
        .await
        .and_then(|v| crate::session_activity::parse_opencode(&v))
    {
        Ok(a) => {
            out.active_sessions = Some(a);
            successes += 1;
        }
        Err(_) => missing = true,
    }
    if successes == 0 {
        return Err(
            "OpenCode V2 status APIs unavailable; no fallback commands are attempted.".into(),
        );
    }
    context.note=if missing {"Some OpenCode status or Context APIs are unavailable; missing data is unknown."}else{"API-reported global Context; no project selected. MCP status is reported, not independently probed."}.into();
    out.context = Some(context);
    out.message=Some("OpenCode local session totals only; daily activity counts steps, not tokens. Quotas cover Go and supported Console budgets only, not every provider. Missing endpoints or per-model budgets remain unknown.".into());
    Ok(out)
}
fn count(v: &Value) -> Result<u64, String> {
    v.as_u64()
        .filter(|n| *n <= 9_007_199_254_740_991)
        .ok_or_else(|| INVALID.into())
}
fn model_id(v: &Value) -> Result<String, String> {
    let s = v
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or(INVALID)?;
    if s.split('/').any(|p| p.is_empty() || p == "." || p == "..")
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:+/".contains(&b))
    {
        return Err(INVALID.into());
    }
    Ok(s.into())
}
fn sum(a: u64, b: u64) -> Result<u64, String> {
    a.checked_add(b)
        .filter(|n| *n <= 9_007_199_254_740_991)
        .ok_or_else(|| INVALID.into())
}
fn tokens(t: &Value) -> Result<(u64, u64, u64, u64, u64, u64), String> {
    let input = count(&t["input"])?;
    let output = sum(count(&t["output"])?, count(&t["reasoning"])?)?;
    let cache_read = count(&t["cache"]["read"])?;
    let cache_write = count(&t["cache"]["write"])?;
    let cached = sum(cache_read, cache_write)?;
    Ok((
        input,
        output,
        cached,
        cache_write,
        cache_read,
        sum(sum(input, output)?, cached)?,
    ))
}
pub fn parse_stats(v: &Value) -> Result<Observation, String> {
    let d = &v["data"];
    let from = cmd::epoch(&d["range"]["from"])?;
    let to = cmd::epoch(&d["range"]["to"])?;
    if d["range"]["from"].as_i64() > d["range"]["to"].as_i64() {
        return Err(INVALID.into());
    }
    let (input, output, cached, cache_creation, cache_read, total) = tokens(&d["tokens"])?;
    let cost = d["cost"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or(INVALID)?;
    let mut models = BTreeMap::new();
    for row in cmd::rows(&d["models"])? {
        let id = model_id(&row["model"]["id"])?;
        let provider = cmd::label(&row["model"]["providerID"])?;
        let name = format!("{provider}:{id}");
        let n = tokens(&row["tokens"])?.5;
        let old = models.entry(name).or_insert(0);
        *old = sum(*old, n)?;
    }
    Ok(Observation{usage:Some(Usage{input_tokens:input,output_tokens:output,cached_tokens:cached,cache_creation_tokens:Some(cache_creation),cache_read_tokens:Some(cache_read),total_tokens:total,spend_usd:Some(cost),source:"OpenCode local session statistics (reasoning included in output; cache read and write included)".into(),scope:"device".into(),as_of:crate::dto::now(),period_start:Some(from),period_end:Some(to)}),model_usage:models.into_iter().map(|(model,total_tokens)|ModelUsage{model,total_tokens}).collect(),..Default::default()})
}
pub fn parse_limits(v: &Value) -> Result<Vec<Quota>, String> {
    let d = v.get("data").ok_or(INVALID)?;
    if d.is_null() {
        return Ok(vec![]);
    }
    if !d.is_object() {
        return Err(INVALID.into());
    }
    let mut quotas = vec![];
    if let Some(go) = d.get("go") {
        for key in ["rolling", "weekly", "monthly"] {
            let m = &go[key];
            let used = cmd::percent(&m["percent"])?;
            if !m["limited"].is_boolean() {
                return Err(INVALID.into());
            }
            quotas.push(Quota {
                id: format!("opencode-go:{key}"),
                label: format!("OpenCode Go {key}"),
                remaining_percent: Some(100.0 - used),
                resets_at: Some(cmd::epoch(&m["resetsAt"])?),
                source: "OpenCode Go allowance".into(),
            });
        }
    }
    for (i, b) in cmd::rows(&d["budgets"])?.iter().enumerate() {
        let limit = b["limit"]
            .as_f64()
            .filter(|n| n.is_finite() && *n > 0.0)
            .ok_or(INVALID)?;
        let spent = b["spent"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or(INVALID)?;
        // A per-model budget's top-level spend is not the allowance of each model.
        let remaining = match b["per"].as_str() {
            Some("set") => Some((100.0 * (1.0 - spent / limit)).clamp(0.0, 100.0)),
            Some("model") => None,
            _ => return Err(INVALID.into()),
        };
        quotas.push(Quota {
            id: format!("opencode-console:{i}"),
            label: format!("OpenCode Console budget {}", i + 1),
            remaining_percent: remaining,
            resets_at: Some(cmd::epoch(&b["resetsAt"])?),
            source: "OpenCode Console budget (not provider-wide)".into(),
        });
    }
    Ok(quotas)
}
pub fn parse_context(mcps: Option<&Value>, skills: Option<&Value>) -> Result<ContextInfo, String> {
    let mut out = ContextInfo::default();
    let mut seen = BTreeSet::new();
    if let Some(v) = mcps {
        for m in cmd::rows(&v["data"])? {
            let name = cmd::label(&m["name"])?;
            let status = m["status"]["status"]
                .as_str()
                .filter(|s| {
                    ["connected", "pending", "disabled", "failed", "needs_auth"].contains(s)
                })
                .ok_or(INVALID)?;
            if !seen.insert(name.clone()) {
                return Err(INVALID.into());
            }
            out.mcps.push(McpInfo {
                name,
                status: status.into(),
            });
        }
    }
    let mut names = BTreeSet::new();
    if let Some(v) = skills {
        for s in cmd::rows(&v["data"])? {
            names.insert(cmd::label(&s["name"])?);
        }
    }
    out.skills = names.into_iter().collect();
    out.note = "API-reported metadata; paths, skill content, and errors omitted.".into();
    Ok(out)
}
