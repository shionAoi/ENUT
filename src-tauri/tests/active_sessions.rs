use ai_usage_monitor::{
    session_activity,
    status_command::{CommandFuture, CommandRunner, Operation},
};
use serde_json::{json, Value};

#[test]
fn opencode_complete_running_map_or_confirmed_zero_only() {
    let a =
        session_activity::parse_opencode(&json!({"data":{"ses_safe":{"type":"running"}}})).unwrap();
    assert_eq!(a.sessions.len(), 1);
    assert_eq!(a.sessions[0].id, "ses_safe");
    assert_eq!(a.sessions[0].state, "running");
    assert!(!a.source.is_empty() && a.scope.contains("service") && !a.as_of.is_empty());
    assert!(session_activity::parse_opencode(&json!({"data":{}}))
        .unwrap()
        .sessions
        .is_empty());
    for v in [
        json!({}),
        json!({"data":null}),
        json!({"data":[]}),
        json!({"data":{"ses_x":null}}),
        json!({"data":{"ses_x":{"type":"idle"}}}),
        json!({"data":{"ses_x":{"type":"running"},"/private/path":{"type":"running"}}}),
    ] {
        assert!(session_activity::parse_opencode(&v).is_err(), "{v}");
    }
    let map: serde_json::Map<String, Value> = (0..257)
        .map(|i| (format!("ses_{i}"), json!({"type":"running"})))
        .collect();
    assert!(session_activity::parse_opencode(&json!({"data":map})).is_err());
    let map: serde_json::Map<String, Value> = (0..256)
        .map(|i| (format!("ses_{i}"), json!({"type":"running"})))
        .collect();
    assert_eq!(
        session_activity::parse_opencode(&json!({"data":map}))
            .unwrap()
            .sessions
            .len(),
        256
    );
}

#[test]
fn claude_only_busy_native_processes_are_running_and_private_fields_are_omitted() {
    let a=session_activity::parse_claude(&json!([
        {"cwd":"/PRIVATE","kind":"interactive","startedAt":100,"pid":1,"status":"busy","sessionId":"uuid-safe","name":"SECRET"},
        {"cwd":"/PRIVATE","kind":"background","startedAt":100,"id":"bg-safe","state":"working","pid":2,"status":"busy"},
        {"cwd":"/PRIVATE","kind":"background","startedAt":100,"id":"sleeping-private-id","state":"working","pid":3,"status":"idle"},
        {"cwd":"/PRIVATE","kind":"background","startedAt":100,"id":"blocked","state":"blocked","pid":4,"status":"waiting","waitingFor":"permission prompt"}
    ])).unwrap();
    assert_eq!(a.sessions.len(), 2);
    let s = serde_json::to_string(&a).unwrap();
    assert!(!s.contains("PRIVATE") && !s.contains("SECRET") && !s.contains("sleeping-private-id"));
    assert!(session_activity::parse_claude(&json!([]))
        .unwrap()
        .sessions
        .is_empty());
    for v in [
        json!({}),
        json!([{}]),
        json!([{"cwd":"/x","kind":"interactive","startedAt":1,"status":"invented","pid":1}]),
        json!([{"cwd":"/x","kind":"background","startedAt":1,"id":"safe","state":"unknown"}]),
    ] {
        assert!(session_activity::parse_claude(&v).is_err());
    }
    let rows:Vec<Value>=(0..257).map(|i|json!({"cwd":"/PRIVATE","kind":"background","startedAt":100,"id":format!("bg-{i}"),"state":"working","pid":i+1,"status":"busy"})).collect();
    assert!(session_activity::parse_claude(&json!(rows)).is_err());
}

#[test]
fn busy_claude_session_can_omit_the_documented_optional_uuid() {
    let a=session_activity::parse_claude(&json!([
        {"cwd":"/PRIVATE","kind":"interactive","startedAt":100,"pid":73,"status":"busy"},
        {"cwd":"/PRIVATE","kind":"interactive","startedAt":100,"pid":74,"status":"busy","sessionId":"uuid-safe"}
    ])).unwrap();
    assert_eq!(a.sessions.len(), 2);
    assert_eq!(a.sessions[0].id, "process-73");
    assert_eq!(a.sessions[1].id, "uuid-safe");
    let duplicate = json!([
        {"cwd":"/PRIVATE","kind":"interactive","startedAt":100,"pid":73,"status":"busy"},
        {"cwd":"/PRIVATE","kind":"interactive","startedAt":100,"pid":73,"status":"busy"}
    ]);
    assert!(session_activity::parse_claude(&duplicate).is_err());
    assert!(!serde_json::to_string(&a).unwrap().contains("PRIVATE"));
}

struct Fake(Value);
impl CommandRunner for Fake {
    fn run(&self, op: Operation) -> CommandFuture<'_> {
        Box::pin(async move {
            if op == Operation::OpenCodeActiveSessions {
                Ok(self.0.to_string().into_bytes())
            } else {
                Err("PRIVATE".into())
            }
        })
    }
}
#[tokio::test]
async fn session_endpoint_can_succeed_without_token_or_quota_endpoints() {
    let o = ai_usage_monitor::opencode_status::read_with(&Fake(json!({"data":{}})))
        .await
        .unwrap();
    assert!(o.usage.is_none() && o.quotas.is_empty());
    assert!(o.active_sessions.unwrap().sessions.is_empty());
    assert!(
        ai_usage_monitor::opencode_status::read_with(&Fake(json!({"data":null})))
            .await
            .is_err()
    );
}
