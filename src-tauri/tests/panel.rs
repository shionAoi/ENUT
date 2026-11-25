use ai_usage_monitor::panel::{close_hides_window, tooltip};
use ai_usage_monitor::Snapshot;

#[test]
fn close_only_hides_when_a_working_panel_icon_keeps_the_app_accessible() {
    assert!(close_hides_window(true));
    assert!(!close_hides_window(false));
}

#[test]
fn panel_tooltip_does_not_invent_quota_without_a_connected_source() {
    let snapshot = Snapshot {
        agents: vec![],
        scanned_at: String::new(),
        platform: "test".into(),
        warnings: vec![],
        billing: vec![],
    };
    assert_eq!(tooltip(&snapshot), "EZTYN AI MONITOR · 0 detected tools");
}

#[test]
fn panel_uses_only_visible_gemini_windows_and_preserves_other_tools() {
    use ai_usage_monitor::{Discovery, LocalStatus, Quota, Service};
    struct Installed;
    impl Discovery for Installed {
        fn installed(&self, _: &str) -> bool {
            true
        }
    }
    let mut snapshot = Service::new(Installed, LocalStatus).scan_agents();
    let agent = snapshot
        .agents
        .iter_mut()
        .find(|a| a.id == "antigravity")
        .unwrap();
    agent.connected = true;
    agent.monitoring_allowed = true;
    let quota = |label: &str, value| Quota {
        id: label.into(),
        label: label.into(),
        remaining_percent: Some(value),
        resets_at: None,
        source: "Synthetic".into(),
    };
    agent.quotas = vec![
        quota("Gemini Pro · Pro · 5h", 80.0),
        quota("Claude · Premium · weekly", 0.0),
        quota("Gemini · Daily · daily", 1.0),
    ];
    assert!(tooltip(&snapshot).contains("80% left"));
    let agent = snapshot
        .agents
        .iter_mut()
        .find(|a| a.id == "codex")
        .unwrap();
    agent.connected = true;
    agent.monitoring_allowed = true;
    agent.quotas = vec![quota("Session", 42.0)];
    assert!(tooltip(&snapshot).contains("42% left"));
    snapshot
        .agents
        .iter_mut()
        .find(|a| a.id == "codex")
        .unwrap()
        .quotas = vec![quota("Session", 42.3456)];
    assert!(tooltip(&snapshot).contains("42.35% left"));
    for agent in &mut snapshot.agents {
        agent.connected = false;
    }
    assert!(!tooltip(&snapshot).contains("% left"));
}

#[test]
fn tray_rows_list_reported_allowances_only_while_approved_connected_and_installed() {
    use ai_usage_monitor::{panel::quota_rows, Discovery, LocalStatus, Quota, Service};
    struct Installed;
    impl Discovery for Installed {
        fn installed(&self, _: &str) -> bool {
            true
        }
    }
    let mut snapshot = Service::new(Installed, LocalStatus).scan_agents();
    let quota = |label: &str, remaining_percent| Quota {
        id: "native-id".into(),
        label: label.into(),
        remaining_percent,
        resets_at: None,
        source: "Synthetic".into(),
    };
    for agent in &mut snapshot.agents {
        agent.connected = true;
        agent.monitoring_allowed = true;
        agent.quotas = match agent.id.as_str() {
            "antigravity" => vec![
                quota("Gemini Pro · Fast · 5h", Some(100.0)),
                quota("Claude · Premium · weekly", Some(0.0)),
                quota("Other · Standard · daily", Some(37.546)),
            ],
            "codex" => vec![
                quota("Plus · 300-minute window", Some(62.0)),
                quota("Unknown", None),
                quota("Invalid", Some(f64::NAN)),
            ],
            _ => vec![],
        };
    }
    assert_eq!(
        quota_rows(&snapshot),
        [
            "Codex · Plus · 5-hour window: 62% left",
            "Antigravity · Gemini Pro · Fast · 5-hour window: 100% left",
            "Antigravity · Claude · Premium · weekly: 0% left",
            "Antigravity · Other · Standard · daily: 37.55% left",
        ]
    );
    for (installed, allowed, connected) in [
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        for agent in &mut snapshot.agents {
            agent.installed = installed;
            agent.monitoring_allowed = allowed;
            agent.connected = connected;
        }
        assert!(quota_rows(&snapshot).is_empty());
        assert!(!tooltip(&snapshot).contains("% left"));
    }
}
