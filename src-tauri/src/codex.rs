//! Documented non-inference Codex app-server status protocol.
//! https://developers.openai.com/codex/app-server/
use crate::{discovery::installed_path, Observation, Quota};
use chrono::{DateTime, SecondsFormat};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

const UNAVAILABLE: &str = "Codex quota unavailable. Check that the installed Codex version supports app-server and is signed in with ChatGPT. No model request was made.";
const MAX_LINE: usize = 256 * 1024;

/// The complete allowlist. No caller can supply arbitrary RPC methods.
pub fn status_requests() -> [Value; 4] {
    [
        json!({"id":0,"method":"initialize","params":{"clientInfo":{"name":"ai_usage_monitor","title":"EZTYN AI MONITOR","version":env!("CARGO_PKG_VERSION")}}}),
        json!({"method":"initialized","params":{}}),
        json!({"id":1,"method":"account/read","params":{"refreshToken":false}}),
        json!({"id":2,"method":"account/rateLimits/read"}),
    ]
}

pub fn parse_rate_limits(result: &Value) -> Result<Vec<Quota>, String> {
    let mut quotas = vec![];
    if let Some(buckets) = result["rateLimitsByLimitId"]
        .as_object()
        .filter(|m| !m.is_empty())
    {
        if buckets.len() > 64 {
            return Err(UNAVAILABLE.into());
        }
        for (id, bucket) in buckets {
            add_windows(id, bucket, &mut quotas)?;
        }
    } else {
        add_windows("codex", &result["rateLimits"], &mut quotas)?;
    }
    if quotas.is_empty() {
        Err(UNAVAILABLE.into())
    } else {
        Ok(quotas)
    }
}

fn add_windows(id: &str, bucket: &Value, quotas: &mut Vec<Quota>) -> Result<(), String> {
    if id.len() > 256 {
        return Err(UNAVAILABLE.into());
    }
    let name = bucket["limitName"]
        .as_str()
        .filter(|s| s.len() <= 256)
        .unwrap_or(id);
    for window in ["primary", "secondary"] {
        let value = &bucket[window];
        if !value.is_object() {
            continue;
        }
        let remaining_percent = match value.get("usedPercent").filter(|v| !v.is_null()) {
            None => None,
            Some(used) => {
                let used = used
                    .as_f64()
                    .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
                    .ok_or(UNAVAILABLE)?;
                Some(100.0 - used)
            }
        };
        let resets_at = value["resetsAt"]
            .as_i64()
            .filter(|s| *s >= 0)
            .and_then(|s| DateTime::from_timestamp(s, 0))
            .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true));
        let duration = value["windowDurationMins"].as_u64().filter(|n| *n > 0);
        let label = match duration {
            Some(mins) => format!("{name} · {mins}-minute window"),
            None => format!("{name} · {window} window"),
        };
        quotas.push(Quota {
            id: format!("{id}:{window}"),
            label,
            remaining_percent,
            resets_at,
            source: "Codex account/rateLimits/read".into(),
        });
    }
    Ok(())
}

async fn send<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(message).map_err(|_| UNAVAILABLE)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(|_| UNAVAILABLE)?;
    writer.flush().await.map_err(|_| UNAVAILABLE.into())
}

async fn response<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    id: u64,
) -> Result<Value, String> {
    let mut total = 0usize;
    for _ in 0..128 {
        // fill_buf/consume bounds allocations even if an untrusted process omits newlines.
        let mut line = vec![];
        loop {
            let bytes = reader.fill_buf().await.map_err(|_| UNAVAILABLE)?;
            if bytes.is_empty() {
                return Err(UNAVAILABLE.into());
            }
            let n = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| i + 1);
            total += n;
            if line.len() + n > MAX_LINE || total > 1024 * 1024 {
                return Err(UNAVAILABLE.into());
            }
            let done = bytes[n - 1] == b'\n';
            line.extend_from_slice(&bytes[..n]);
            reader.consume(n);
            if done {
                break;
            }
        }
        let value: Value = serde_json::from_slice(&line).map_err(|_| UNAVAILABLE)?;
        if value["id"].as_u64() != Some(id) {
            continue;
        }
        if value.get("error").is_some() || value.get("result").is_none() {
            return Err(UNAVAILABLE.into());
        }
        return Ok(value["result"].clone());
    }
    Err(UNAVAILABLE.into())
}

/// Public for deterministic in-memory protocol tests; never reads installed credentials.
pub async fn exchange<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    input: R,
    mut output: W,
) -> Result<Observation, String> {
    let mut input = BufReader::new(input);
    let messages = status_requests();
    send(&mut output, &messages[0]).await?;
    response(&mut input, 0).await?;
    send(&mut output, &messages[1]).await?;
    send(&mut output, &messages[2]).await?;
    let account = response(&mut input, 1).await?;
    if account.get("account").is_some_and(Value::is_null)
        && account["requiresOpenaiAuth"].as_bool() == Some(true)
    {
        return Ok(Observation { authentication: Some("required".into()), message: Some("Codex reports authentication is required. Login uses a device code on the provider website; no subscription or token usage has been reported.".into()), ..Default::default() });
    }
    match account["account"]["type"].as_str() {
        Some("apiKey") => return Ok(Observation { authentication: Some("available".into()), message: Some("Codex reports API-key authentication. This is not a signed-out account; ChatGPT subscription quotas and local token totals are unavailable from this account-status source.".into()), ..Default::default() }),
        Some("chatgpt") => {},
        _ => return Err("Codex subscription quotas require an existing ChatGPT sign-in; API-key mode has no subscription balance.".into()),
    }
    send(&mut output, &messages[3]).await?;
    Ok(Observation {
        authentication: Some("available".into()),
        models: vec![],
        quotas: parse_rate_limits(&response(&mut input, 2).await?)?,
        ..Default::default()
    })
}

fn launchable(path: &Path) -> bool {
    // Never use cmd.exe/shell interpolation for npm .cmd wrappers on Windows.
    path.is_absolute()
        && (!cfg!(windows)
            || path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")))
}

pub(crate) fn command(path: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(path);
    #[cfg(unix)]
    if let Some(bin) = path.parent().filter(|p| p.is_absolute()) {
        let mut paths = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .unwrap_or_default();
        if !paths.iter().any(|p| p == bin) {
            // npm Codex uses /usr/bin/env node; GUI launches may lack fnm's bin.
            paths.insert(0, bin.to_path_buf());
            if let Ok(value) = std::env::join_paths(paths) {
                command.env("PATH", value);
            }
        }
    }
    command
}

pub async fn read() -> Result<Observation, String> {
    let path = installed_path("codex")
        .filter(|p| launchable(p))
        .ok_or(UNAVAILABLE)?;
    let mut child = command(&path)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| UNAVAILABLE)?;
    let output = child.stdin.take().ok_or(UNAVAILABLE)?;
    let input = child.stdout.take().ok_or(UNAVAILABLE)?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), exchange(input, output))
        .await
        .unwrap_or_else(|_| Err(UNAVAILABLE.into()));
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}
