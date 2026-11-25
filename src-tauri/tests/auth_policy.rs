use ai_usage_monitor::auth::{choices_for, login_args, parse_integrations, valid_browser_url};
use serde_json::json;
#[test]
fn native_choices_are_explicit_and_local_tools_require_no_login() {
    assert_eq!(choices_for("codex").unwrap()[0].kind, "device_code");
    assert!(choices_for("ollama").unwrap().is_empty());
    assert!(choices_for("lmstudio").unwrap().is_empty());
    assert!(choices_for("unknown").is_err());
    let c = choices_for("hermes").unwrap();
    assert!(c.iter().any(|c| c.provider == "anthropic"));
    assert_eq!(
        login_args("antigravity", &choices_for("antigravity").unwrap()[0]).unwrap(),
        vec!["acp", "--login"]
    );
}
#[test]
fn provider_methods_come_from_safe_oauth_discovery_not_frontend_arguments() {
    let v = json!({"data":[{"id":"openai","name":"OpenAI","methods":[{"id":"browser","type":"oauth","label":"Browser"},{"id":"key","type":"api","label":"Key"}],"connections":[{"token":"SECRET"}]},{"id":"bad/path","name":"Bad","methods":[]}]});
    let c = parse_integrations(&v).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].provider, "openai");
    assert_eq!(
        login_args("opencode", &c[0]).unwrap(),
        vec!["auth", "login", "openai", "--method", "browser"]
    );
    assert!(!serde_json::to_string(&c).unwrap().contains("SECRET"));
}
#[test]
fn browser_opening_has_no_frontend_url_or_shell_escape() {
    assert!(valid_browser_url("https://auth.openai.com/codex/device"));
    for v in [
        "http://auth.openai.com/codex/device",
        "https://auth.openai.com.evil/codex/device",
        "https://user@auth.openai.com/codex/device",
        "file:///private",
        "javascript:alert(1)",
        "https://auth.openai.com:444/codex/device",
    ] {
        assert!(!valid_browser_url(v));
    }
}
