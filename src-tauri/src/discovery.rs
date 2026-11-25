use std::path::PathBuf;

pub trait Discovery: Send + Sync {
    /// Only executable metadata is inspected; never launch a discovered program.
    fn installed(&self, id: &str) -> bool;
    fn running(&self, _id: &str) -> Option<bool> {
        None
    }
}

/// Narrow install checks: PATH, user CLI bins, and vendor-specific locations.
/// No recursive scans, credential files, process environments, or usage logs.
pub struct SystemDiscovery;

pub fn installation_candidates(
    id: &str,
    windows: bool,
    paths: &[PathBuf],
    home: Option<&std::path::Path>,
    local: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let names: &[&str] = match (id, windows) {
        ("codex", true) => &["codex.exe", "codex.cmd"],
        ("claude", true) => &["claude.exe", "claude.cmd"],
        ("gemini", true) => &["gemini.exe", "gemini.cmd"],
        ("antigravity", true) => &["agy.exe", "agy.cmd"],
        ("opencode", true) => &["opencode.exe", "opencode.cmd"],
        ("hermes", true) => &["hermes.exe", "hermes.cmd"],
        ("ollama", true) => &["ollama.exe"],
        ("lmstudio", true) => &["lms.exe", "LM Studio.exe"],
        ("codex", false) => &["codex"],
        ("claude", false) => &["claude"],
        ("gemini", false) => &["gemini"],
        ("antigravity", false) => &["agy"],
        ("opencode", false) => &["opencode"],
        ("hermes", false) => &["hermes"],
        ("ollama", false) => &["ollama"],
        ("lmstudio", false) => &["lms", "lm-studio"],
        _ => return vec![],
    };
    let mut dirs = paths.to_vec();
    if let Some(home) = home {
        if windows {
            dirs.push(home.join("AppData/Roaming/npm"));
            dirs.push(home.join(".local/bin"));
        } else {
            dirs.extend([home.join(".local/bin"), home.join(".npm-global/bin")]);
            if id == "codex" {
                dirs.push(home.join(".local/share/fnm/aliases/default/bin"));
            }
        }
        if id == "lmstudio" {
            dirs.push(home.join(".lmstudio/bin"));
        }
        if id == "hermes" && !windows {
            dirs.push(home.join(".hermes/venv/bin"));
        }
    }
    if !windows {
        dirs.extend([PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin")]);
        if id == "lmstudio" {
            dirs.push(PathBuf::from("/opt/LM Studio"));
        }
    } else if let Some(local) = local {
        match id {
            "ollama" => dirs.push(local.join("Programs/Ollama")),
            "lmstudio" => dirs.push(local.join("Programs/LM Studio")),
            _ => {}
        }
    }
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .collect()
}

impl Discovery for SystemDiscovery {
    fn installed(&self, id: &str) -> bool {
        installed_path(id).is_some()
    }
    fn running(&self, id: &str) -> Option<bool> {
        running_apps()
            .is_some_and(|apps| apps.contains(id))
            .then_some(true)
    }
}

pub fn matches_process(id: &str, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    match id {
        "ollama" => name == "ollama",
        "lmstudio" => matches!(
            name,
            "lm studio" | "lm-studio" | "lmstudio" | "llmster" | "lms"
        ),
        "claude" => name == "claude",
        "codex" => name == "codex",
        "gemini" => name == "gemini",
        "antigravity" => matches!(name, "agy" | "antigravity"),
        "opencode" => name == "opencode",
        "hermes" => name == "hermes",
        _ => false,
    }
}

fn running_apps() -> Option<std::collections::HashSet<&'static str>> {
    use std::{
        collections::HashSet,
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };
    static CACHE: OnceLock<Mutex<Option<(Instant, HashSet<&'static str>)>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(|| Mutex::new(None)).lock().ok()?;
    if let Some((when, apps)) = &*cache {
        if when.elapsed() < Duration::from_secs(15) {
            return Some(apps.clone());
        }
    }
    // Minimal process refresh excludes arguments, environment, users, executable paths and tasks.
    // Source: https://github.com/GuillaumeGomez/sysinfo/blob/main/_autodocs/api-reference/RefreshKinds.md
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().without_tasks(),
    );
    let mut apps = HashSet::new();
    for process in system.processes().values() {
        let name = process.name().to_string_lossy();
        for id in [
            "codex",
            "claude",
            "antigravity",
            "gemini",
            "opencode",
            "hermes",
            "ollama",
            "lmstudio",
        ] {
            if matches_process(id, &name) {
                apps.insert(id);
            }
        }
    }
    *cache = Some((Instant::now(), apps.clone()));
    Some(apps)
}

pub fn installed_path(id: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")
        .map(|v| {
            std::env::split_paths(&v)
                .filter(|p| p.is_absolute())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    installation_candidates(id, cfg!(windows), &paths, home.as_deref(), local.as_deref())
        .iter()
        .find(|p| {
            let Ok(meta) = p.metadata() else { return false };
            if !meta.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
        .cloned()
}
