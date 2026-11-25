use ai_usage_monitor::context;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "context-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn put(&self, p: &str, s: &str) {
        let p = self.0.join(p);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn native_config_metadata_is_configured_not_connected() {
    let h = Home::new();
    h.put(
        ".codex/config.toml",
        "[mcp_servers.safe]\ncommand='SECRET'\n",
    );
    h.put(
        ".agents/skills/shared/SKILL.md",
        "---\nname: shared\n---\nPRIVATE",
    );
    let c = context::read_local(&h.0, "codex").unwrap();
    assert_eq!(c.mcps[0].name, "safe");
    assert_eq!(c.mcps[0].status, "configured");
    assert_eq!(c.skills, vec!["shared"]);
    let s = serde_json::to_string(&c).unwrap();
    assert!(!s.contains("SECRET") && !s.contains("PRIVATE") && !s.contains(h.0.to_str().unwrap()));
    h.put(
        ".hermes/config.yaml",
        "mcp_servers:\n  hermes-safe:\n    command: SECRET\n",
    );
    h.put(
        ".hermes/skills/category/native/SKILL.md",
        "---\nname: native\n---\nSECRET",
    );
    let c = context::read_local(&h.0, "hermes").unwrap();
    assert_eq!(c.mcps[0].name, "hermes-safe");
    assert!(c.skills.contains(&"native".into()));
    h.put(
        ".gemini/config/mcp_config.json",
        r#"{"mcpServers":{"agy-safe":{"url":"SECRET"}}}"#,
    );
    assert_eq!(
        context::read_local(&h.0, "antigravity").unwrap().mcps[0].name,
        "agy-safe"
    );
    h.put(
        ".claude.json",
        r#"{"mcpServers":{"claude-safe":{"command":"SECRET"}},"projects":{"PRIVATE":{}}}"#,
    );
    assert_eq!(
        context::read_local(&h.0, "claude").unwrap().mcps[0].name,
        "claude-safe"
    );
}
#[test]
fn absent_context_is_not_a_claim_of_zero_connected_and_unknown_tool_rejected() {
    let h = Home::new();
    let c = context::read_local(&h.0, "codex").unwrap();
    assert!(c.mcps.is_empty() && !c.note.is_empty());
    assert!(context::read_local(&h.0, "unknown").is_err());
}
#[cfg(unix)]
#[test]
fn symlink_directories_and_special_files_never_read() {
    use std::os::unix::fs::symlink;
    let h = Home::new();
    let out = Home::new();
    out.put("config.toml", "[mcp_servers.secret]\ncommand='x'");
    symlink(&out.0, h.0.join(".codex")).unwrap();
    assert!(context::read_local(&h.0, "codex").unwrap().mcps.is_empty());
    h.put(".agents/skills/real/SKILL.md", "---\nname: real\n---");
    symlink(
        h.0.join(".agents/skills/real"),
        h.0.join(".agents/skills/linked"),
    )
    .unwrap();
    assert_eq!(
        context::read_local(&h.0, "codex").unwrap().skills,
        vec!["real"]
    );
}

#[test]
fn unsafe_names_invalid_config_and_oversized_manifests_are_omitted() {
    let h = Home::new();
    h.put(
        ".claude.json",
        r#"{"mcpServers":{"/private/path":{"command":"SECRET"},"valid":{"env":{"KEY":"SECRET"}}}}"#,
    );
    h.put(".claude/skills/invalid/SKILL.md", "not a manifest");
    h.put(".claude/skills/huge/SKILL.md", &"x".repeat(300000));
    let c = context::read_local(&h.0, "claude").unwrap();
    assert_eq!(c.mcps.len(), 1);
    assert_eq!(c.mcps[0].name, "valid");
    assert!(c.skills.is_empty());
    h.put(".codex/config.toml", "bad [toml");
    assert!(context::read_local(&h.0, "codex").unwrap().mcps.is_empty());
}
