//! Permission must be checked by the caller. Fixed home-relative metadata only.
use crate::{
    status_command::{self as cmd, INVALID},
    ContextInfo, McpInfo,
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde_json::Value;
use std::{collections::BTreeSet, io::Read, path::Path};
const MAX_FILE: u64 = 256 * 1024;
const MAX_ENTRIES: usize = 1024;
const NOTE:&str="Home-scoped metadata only; MCPs are configured, not confirmed connected. Missing, linked, invalid, oversized or inaccessible files are omitted, not zero connected servers. Project, plugin, custom paths and profile overrides are not scanned; installed skills are not proof of activation.";

// All filesystem access shares this descriptor-relative, no-follow reader. No config path is followed.
fn directory(root: &Dir, parts: &[&str]) -> Result<Dir, String> {
    let mut d = root.try_clone().map_err(|_| INVALID)?;
    for part in parts {
        d = d.open_dir_nofollow(part).map_err(|_| INVALID)?;
    }
    Ok(d)
}
fn text(root: &Dir, parts: &[&str]) -> Result<String, String> {
    let (file, dirs) = parts.split_last().ok_or(INVALID)?;
    let d = directory(root, dirs)?;
    let mut o = OpenOptions::new();
    o.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NONBLOCK);
    }
    let f = d.open_with(file, &o).map_err(|_| INVALID)?;
    let m = f.metadata().map_err(|_| INVALID)?;
    if !m.is_file() || m.len() > MAX_FILE {
        return Err(INVALID.into());
    }
    let mut out = String::new();
    f.take(MAX_FILE + 1)
        .read_to_string(&mut out)
        .map_err(|_| INVALID)?;
    if out.len() > MAX_FILE as usize {
        return Err(INVALID.into());
    }
    Ok(out)
}
fn config(root: &Dir, id: &str) -> Result<Value, String> {
    match id {
        "codex" => {
            let t: toml::Value =
                toml::from_str(&text(root, &[".codex", "config.toml"])?).map_err(|_| INVALID)?;
            serde_json::to_value(t).map_err(|_| INVALID.into())
        }
        "claude" => {
            serde_json::from_str(&text(root, &[".claude.json"])?).map_err(|_| INVALID.into())
        }
        "antigravity" => {
            serde_json::from_str(&text(root, &[".gemini", "config", "mcp_config.json"])?)
                .map_err(|_| INVALID.into())
        }
        "hermes" => {
            let t: serde_yaml::Value =
                serde_yaml::from_str(&text(root, &[".hermes", "config.yaml"])?)
                    .map_err(|_| INVALID)?;
            serde_json::to_value(t).map_err(|_| INVALID.into())
        }
        _ => Err(INVALID.into()),
    }
}
fn skills(
    d: &Dir,
    depth: usize,
    entries: &mut usize,
    names: &mut BTreeSet<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    check_cancelled(cancelled)?;
    if let Ok(manifest) = text(d, &["SKILL.md"]) {
        // Only validated folder metadata is displayed; never parse or publish instructions.
        if manifest.starts_with("---\n") || manifest.starts_with("---\r\n") {
            return Ok(());
        }
    }
    for entry in d.entries().map_err(|_| INVALID)? {
        check_cancelled(cancelled)?;
        *entries += 1;
        if *entries > MAX_ENTRIES {
            return Err(INVALID.into());
        }
        let e = entry.map_err(|_| INVALID)?;
        let name = e.file_name();
        let Some(s) = name.to_str() else {
            continue;
        };
        if s.starts_with('.') || cmd::label(&Value::String(s.into())).is_err() {
            continue;
        }
        let Ok(child) = d.open_dir_nofollow(&name) else {
            continue;
        };
        if let Ok(manifest) = text(&child, &["SKILL.md"]) {
            if manifest.starts_with("---\n") || manifest.starts_with("---\r\n") {
                if names.len() >= 256 {
                    return Err(INVALID.into());
                }
                names.insert(s.into());
                continue;
            }
        }
        if depth < 2 {
            skills(&child, depth + 1, entries, names, cancelled)?;
        }
    }
    Ok(())
}
pub fn read_local(home: &Path, id: &str) -> Result<ContextInfo, String> {
    read_local_until(home, id, &|| false)
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        return Err("Context discovery was cancelled.".into());
    }
    Ok(())
}

pub(crate) fn read_local_until(
    home: &Path,
    id: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<ContextInfo, String> {
    check_cancelled(cancelled)?;
    if !["codex", "claude", "antigravity", "hermes"].contains(&id) {
        return Err("Unsupported Context integration.".into());
    }
    if !home.is_absolute() {
        return Err("Context home unavailable.".into());
    }
    let root = Dir::open_ambient_dir(home, ambient_authority())
        .map_err(|_| "Context home unavailable.")?;
    let mut out = ContextInfo {
        note: NOTE.into(),
        ..Default::default()
    };
    if let Ok(v) = config(&root, id) {
        let key = if id == "codex" || id == "hermes" {
            "mcp_servers"
        } else {
            "mcpServers"
        };
        if let Some(map) = v[key].as_object().filter(|m| m.len() <= 256) {
            for (name, entry) in map {
                if !entry.is_object() {
                    continue;
                }
                if let Ok(name) = cmd::label(&Value::String(name.clone())) {
                    out.mcps.push(McpInfo {
                        name,
                        status: "configured".into(),
                    });
                }
            }
        }
    }
    let mut paths: Vec<&[&str]> = vec![&[".agents", "skills"]];
    match id {
        "codex" => paths.push(&[".codex", "skills"]),
        "claude" => paths.push(&[".claude", "skills"]),
        "hermes" => paths.push(&[".hermes", "skills"]),
        "antigravity" => {
            paths.push(&[".gemini", "config", "skills"]);
            paths.push(&[".gemini", "antigravity-cli", "skills"]);
            paths.push(&[".gemini", "skills"]);
        }
        _ => {}
    }
    let mut names = BTreeSet::new();
    let mut entries = 0;
    for p in paths {
        check_cancelled(cancelled)?;
        if let Ok(d) = directory(&root, p) {
            if skills(&d, 0, &mut entries, &mut names, cancelled).is_err() {
                check_cancelled(cancelled)?;
                out.note =
                    format!("{NOTE} Discovery safety limit reached; list may be incomplete.");
                break;
            }
        }
    }
    out.skills = names.into_iter().collect();
    Ok(out)
}
