//! Native quota print mode only; envelope counters are not historical usage.
use crate::{
    status_command::{self as cmd, CommandRunner, Operation, ProcessRunner, INVALID},
    Observation, Quota,
};
use serde_json::Value;
use std::collections::BTreeSet;
pub const KEYRING_LOCKED: &str =
    "Desktop keyring is locked; Antigravity quota will load automatically once unlocked.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyringState {
    Ready,
    Locked,
}

pub fn parse_keyring_locked(stdout: &[u8]) -> Option<bool> {
    match std::str::from_utf8(stdout).ok()?.trim() {
        "b true" | "(<true>,)" => Some(true),
        "b false" | "(<false>,)" => Some(false),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
async fn probe_keyring_command(program: &str, args: &[&str]) -> Result<Vec<u8>, std::io::ErrorKind> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| e.kind())?;
    let stdout = child.stdout.take().ok_or(std::io::ErrorKind::Other)?;
    let work = async {
        let read = async {
            let mut buf = Vec::new();
            stdout
                .take(1024)
                .read_to_end(&mut buf)
                .await
                .map_err(|e| e.kind())?;
            Ok::<_, std::io::ErrorKind>(buf)
        };
        let wait = async { child.wait().await.map_err(|e| e.kind()) };
        let (out, status) = tokio::try_join!(read, wait)?;
        if !status.success() {
            return Err(std::io::ErrorKind::Other);
        }
        Ok(out)
    };
    match tokio::time::timeout(std::time::Duration::from_secs(2), work).await {
        Ok(res) => res,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(std::io::ErrorKind::TimedOut)
        }
    }
}

#[cfg(target_os = "linux")]
pub async fn linux_keyring_state() -> KeyringState {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none_or(|v| v.is_empty()) {
        return KeyringState::Ready;
    }
    let res = match probe_keyring_command(
        "/usr/bin/busctl",
        &[
            "--user",
            "get-property",
            "org.freedesktop.secrets",
            "/org/freedesktop/secrets/aliases/default",
            "org.freedesktop.Secret.Collection",
            "Locked",
        ],
    )
    .await
    {
        Err(std::io::ErrorKind::NotFound) => {
            probe_keyring_command(
                "/usr/bin/gdbus",
                &[
                    "call",
                    "--session",
                    "--dest",
                    "org.freedesktop.secrets",
                    "--object-path",
                    "/org/freedesktop/secrets/aliases/default",
                    "--method",
                    "org.freedesktop.DBus.Properties.Get",
                    "org.freedesktop.Secret.Collection",
                    "Locked",
                ],
            )
            .await
        }
        other => other,
    };
    match res {
        Err(std::io::ErrorKind::TimedOut) => KeyringState::Locked,
        Ok(out) if parse_keyring_locked(&out) == Some(true) => KeyringState::Locked,
        _ => KeyringState::Ready,
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn linux_keyring_state() -> KeyringState {
    KeyringState::Ready
}

pub async fn read() -> Result<Observation, String> {
    read_with_keyring(&ProcessRunner::default(), linux_keyring_state().await).await
}
pub async fn read_with(r: &dyn CommandRunner) -> Result<Observation, String> {
    read_with_keyring(r, KeyringState::Ready).await
}
pub async fn read_with_keyring(
    r: &dyn CommandRunner,
    keyring: KeyringState,
) -> Result<Observation, String> {
    if keyring == KeyringState::Locked {
        return Err(KEYRING_LOCKED.into());
    }
    let version = r.run(Operation::AntigravityVersion).await?;
    if !supported_version(&version) {
        return Err("Antigravity quota requires a recognized stable CLI version >=1.1.11; no prompt fallback is attempted.".into());
    }
    match cmd::json(r, Operation::AntigravityUsage).await {
        Ok(value) => parse(&value),
        Err(err) if err == cmd::AUTH_REQUIRED => Ok(Observation {
            authentication: Some("required".into()),
            message: Some(err),
            ..Default::default()
        }),
        Err(err) => Err(err),
    }
}
fn supported_version(b: &[u8]) -> bool {
    if b.len() > 128 {
        return false;
    }
    let Ok(s) = std::str::from_utf8(b) else {
        return false;
    };
    let s = s.trim_end_matches(['\r', '\n']);
    let parts = s.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return false;
    }
    let mut nums = vec![];
    for p in parts {
        if p.is_empty()
            || (p.len() > 1 && p.starts_with('0'))
            || !p.bytes().all(|b| b.is_ascii_digit())
        {
            return false;
        }
        let Ok(n) = p.parse::<u32>() else {
            return false;
        };
        nums.push(n);
    }
    nums.as_slice() >= [1, 1, 11].as_slice()
}
pub fn parse(v: &Value) -> Result<Observation, String> {
    let mut quotas = vec![];
    let mut seen = BTreeSet::new();
    for group in cmd::rows(&v["command"]["data"]["groups"])? {
        let g = cmd::label(&group["name"])?;
        for b in cmd::rows(&group["buckets"])? {
            if quotas.len() >= 256 {
                return Err(INVALID.into());
            }
            let id = cmd::label(&b["id"])?;
            let name = cmd::label(&b["name"])?;
            let window = cmd::label(&b["window"])?;
            let fraction = b["remaining_fraction"]
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
                .ok_or(INVALID)?;
            let reset = cmd::reset(&b["reset_time"])?.ok_or(INVALID)?;
            let key = format!("{}:{g}{}:{id}{}:{window}", g.len(), id.len(), window.len());
            if !seen.insert(key.clone()) {
                return Err(INVALID.into());
            }
            quotas.push(Quota {
                id: key,
                label: format!("{g} · {name} · {window}"),
                remaining_percent: Some(fraction * 100.0),
                resets_at: Some(reset),
                source: "Antigravity native quota".into(),
            });
        }
    }
    if quotas.is_empty() {
        return Err(INVALID.into());
    }
    Ok(Observation {
        quotas,
        message: Some("Account quota only; command token counters are not usage history.".into()),
        ..Default::default()
    })
}
