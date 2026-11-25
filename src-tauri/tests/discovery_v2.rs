use ai_usage_monitor::{discovery::matches_process, installation_candidates};
use std::path::{Path, PathBuf};
#[test]
fn new_tools_have_narrow_native_candidates_and_process_names() {
    for (id, exe) in [
        ("antigravity", "agy"),
        ("opencode", "opencode"),
        ("hermes", "hermes"),
    ] {
        let paths = installation_candidates(
            id,
            false,
            &[PathBuf::from("/fixture/bin")],
            Some(Path::new("/fixture/home")),
            None,
        );
        assert!(paths.contains(&PathBuf::from(format!("/fixture/bin/{exe}"))));
        assert!(matches_process(id, exe));
        assert!(!matches_process(id, "node"));
        assert!(
            installation_candidates(id, true, &[PathBuf::from("C:/fixture")], None, None)
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "exe"))
        );
    }
    // Existing discovery remains metadata-only and does not lose known native runtimes.
    assert!(matches_process("ollama", "ollama.exe"));
    assert!(matches_process("lmstudio", "llmster"));
}

#[test]
fn codex_desktop_discovery_includes_fnm_default_without_changing_other_tools() {
    let home = Path::new("/fixture/home");
    let path = [PathBuf::from("/fixture/explicit")];
    let fnm = home.join(".local/share/fnm/aliases/default/bin/codex");
    let candidates = installation_candidates("codex", false, &path, Some(home), None);
    assert_eq!(candidates[0], PathBuf::from("/fixture/explicit/codex"));
    assert!(candidates.contains(&fnm));
    for tool in [
        "claude",
        "antigravity",
        "opencode",
        "hermes",
        "ollama",
        "lmstudio",
    ] {
        assert!(
            !installation_candidates(tool, false, &path, Some(home), None)
                .iter()
                .any(|p| p.starts_with(home.join(".local/share/fnm")))
        );
    }
    assert!(
        !installation_candidates("codex", true, &path, Some(home), None)
            .iter()
            .any(|p| p.starts_with(home.join(".local/share/fnm")))
    );
}
