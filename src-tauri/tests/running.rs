use ai_usage_monitor::discovery::matches_process;
use ai_usage_monitor::{Discovery, LocalStatus, Service};

#[test]
fn process_names_match_known_apps_without_treating_every_node_as_an_agent() {
    assert!(matches_process("ollama", "ollama.exe"));
    assert!(matches_process("lmstudio", "llmster"));
    assert!(!matches_process("gemini", "node"));
    assert!(!matches_process("codex", "my-codex-unrelated"));
}

#[test]
fn running_local_service_appears_without_a_path_executable_and_without_usage_access() {
    struct ProcessOnly;
    impl Discovery for ProcessOnly {
        fn installed(&self, _: &str) -> bool {
            false
        }
        fn running(&self, id: &str) -> Option<bool> {
            (id == "ollama").then_some(true)
        }
    }
    let service = Service::new(ProcessOnly, LocalStatus);
    let snapshot = service.scan_agents();
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.agents[0].id, "ollama");
    assert_eq!(snapshot.agents[0].running, Some(true));
    assert!(!snapshot.agents[0].connected);
    assert!(snapshot.agents[0].models.is_empty());
}
