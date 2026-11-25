use ai_usage_monitor::process_monitor::{
    build_process_snapshot, direct_agent, sanitize_task_label, RawProcess,
};
use std::collections::HashSet;

fn approved(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn attributes_descendant_tasks_without_double_counting_nested_agents_or_self() {
    let procs = vec![
        // Monitor itself + its status probe must always be excluded
        RawProcess {
            pid: 900,
            parent_pid: Some(1),
            name: "ai-usage-monitor".into(),
            cmd: vec!["ai-usage-monitor".into()],
            memory_bytes: 50 * 1048576,
            cpu_usage_raw: 12.0,
        },
        RawProcess {
            pid: 901,
            parent_pid: Some(900),
            name: "codex".into(),
            cmd: vec!["codex".into(), "status".into()],
            memory_bytes: 20 * 1048576,
            cpu_usage_raw: 8.0,
        },
        // Codex root + sh + cargo + git + nested claude agent
        RawProcess {
            pid: 100,
            parent_pid: Some(1),
            name: "codex".into(),
            cmd: vec!["codex".into()],
            memory_bytes: 3 * 1048576,
            cpu_usage_raw: 6.0,
        },
        RawProcess {
            pid: 101,
            parent_pid: Some(100),
            name: "cargo".into(),
            cmd: vec!["cargo".into(), "test".into(), "--manifest-path".into(), "/home/alice/secret-proj/Cargo.toml".into()],
            memory_bytes: 2 * 1048576,
            cpu_usage_raw: 4.0,
        },
        RawProcess {
            pid: 102,
            parent_pid: Some(101),
            name: "git".into(),
            cmd: vec!["git".into(), "clone".into(), "https://alice:topsecret@github.com/org/repo.git".into()],
            memory_bytes: 1 * 1048576,
            cpu_usage_raw: 2.0,
        },
        // Nested Claude CLI via node wrapper spawned under codex stays isolated under Claude
        RawProcess {
            pid: 200,
            parent_pid: Some(100),
            name: "node".into(),
            cmd: vec![
                "node".into(),
                "/home/alice/.npm/node_modules/@anthropic-ai/claude-code/cli.js".into(),
            ],
            memory_bytes: 4 * 1048576,
            cpu_usage_raw: 8.0,
        },
        // Unrelated node process must not be treated as an agent
        RawProcess {
            pid: 300,
            parent_pid: Some(1),
            name: "node".into(),
            cmd: vec!["node".into(), "/home/alice/app/server.js".into()],
            memory_bytes: 100 * 1048576,
            cpu_usage_raw: 20.0,
        },
        // Antigravity approved vs Gemini CLI unapproved
        RawProcess {
            pid: 400,
            parent_pid: Some(1),
            name: "agy".into(),
            cmd: vec!["agy".into()],
            memory_bytes: 5 * 1048576,
            cpu_usage_raw: 4.0,
        },
        RawProcess {
            pid: 500,
            parent_pid: Some(1),
            name: "gemini".into(),
            cmd: vec!["gemini".into()],
            memory_bytes: 50 * 1048576,
            cpu_usage_raw: 40.0,
        },
    ];

    // 2 logical CPUs -> raw 12.0 on codex (6+4+2) => 6.0%, claude (8) => 4.0%, antigravity (4) => 2.0%
    let snap = build_process_snapshot(
        &procs,
        16 * 1073741824,
        2,
        &approved(&["codex", "claude", "antigravity"]),
        900,
    );

    assert_eq!(snap.agents.len(), 3);
    assert_eq!(snap.agents[0].agent_id, "codex");
    assert_eq!(snap.agents[0].memory_bytes, 6 * 1048576);
    assert!((snap.agents[0].cpu_percent - 6.0).abs() < 0.01);
    assert_eq!(snap.agents[0].processes.len(), 3);
    assert!(!snap.agents[0]
        .processes
        .iter()
        .any(|p| p.pid == 200 || p.pid == 901));

    // Secret URL credentials and home directory path must be stripped from task labels
    let git_task = snap.agents[0]
        .processes
        .iter()
        .find(|p| p.pid == 102)
        .unwrap();
    assert!(!git_task.task.contains("topsecret"));
    assert!(!git_task.task.contains("alice"));
    assert!(git_task.task.starts_with("git clone"));

    let cargo_task = snap.agents[0]
        .processes
        .iter()
        .find(|p| p.pid == 101)
        .unwrap();
    assert!(!cargo_task.task.contains("secret-proj"));
    assert!(cargo_task.task.contains("Cargo.toml"));

    // Total excludes self (900/901), unrelated node (300), and unapproved gemini (500)
    assert_eq!(snap.total_memory_bytes, 15 * 1048576);
    assert!((snap.total_cpu_percent - 12.0).abs() < 0.01);
}

#[test]
fn wrapper_matching_and_task_sanitization_strip_secrets_and_private_paths() {
    let claude = RawProcess {
        pid: 1,
        parent_pid: None,
        name: "node".into(),
        cmd: vec![
            "/usr/bin/node".into(),
            "--max-old-space-size=4096".into(),
            "/opt/tools/@anthropic-ai/claude-code/cli.js".into(),
        ],
        memory_bytes: 1,
        cpu_usage_raw: 0.0,
    };
    assert_eq!(direct_agent(&claude), Some("claude"));

    let vendor_codex = RawProcess {
        pid: 2,
        parent_pid: None,
        name: "codex-x86_64-unknown-linux-musl".into(),
        cmd: vec![],
        memory_bytes: 1,
        cpu_usage_raw: 0.0,
    };
    assert_eq!(direct_agent(&vendor_codex), Some("codex"));

    let unrelated = RawProcess {
        pid: 3,
        parent_pid: None,
        name: "node".into(),
        cmd: vec!["node".into(), "server.js".into()],
        memory_bytes: 1,
        cpu_usage_raw: 0.0,
    };
    assert_eq!(direct_agent(&unrelated), None);

    let sanitized = sanitize_task_label(
        "curl",
        &[
            "/usr/bin/curl".into(),
            "--api-key".into(),
            "sk-live-very-secret-value".into(),
            "TOKEN=supersecret123".into(),
            "https://user:pass123@api.example.com/v1/models?key=abc".into(),
            "/home/shion/work/subagent.js".into(),
        ],
    );
    assert!(!sanitized.contains("sk-live"));
    assert!(!sanitized.contains("supersecret"));
    assert!(!sanitized.contains("pass123"));
    assert!(!sanitized.contains("key=abc"));
    assert!(!sanitized.contains("/home/shion"));
    assert!(sanitized.starts_with("curl "));
}
