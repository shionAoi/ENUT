use ai_usage_monitor::{claude_inputs, local_usage_observation, usage::load_local_usage};
use chrono::Utc;
use serde_json::json;

#[test]
fn mixed_claude_combines_stream_and_ordinary_assistant_records_ccusage_method() {
    let home = std::env::temp_dir().join(format!("widget-mixed-claude-{}", std::process::id()));
    let directory = home.join(".claude/projects/synthetic");
    std::fs::create_dir_all(&directory).unwrap();
    let now = Utc::now();
    let docs = include_str!("../../docs/integrations/claude.md");
    let stream = docs
        .split("```jsonl\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap()
        .replace("2026-11-07T10:00:00Z", &now.to_rfc3339());
    std::fs::write(directory.join("stream.jsonl"), stream).unwrap();
    let ordinary = json!({"type":"assistant","timestamp":now.to_rfc3339(),"requestId":"req_other","message":{"role":"assistant","id":"msg_other","model":"claude-fixture","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":1}}});
    std::fs::write(directory.join("ordinary.jsonl"), ordinary.to_string()).unwrap();
    let summary = load_local_usage(&home, "claude", now).unwrap();
    // stream (10 in + 9 out + 30 cache_create + 20 cache_read = 69) + ordinary (10 + 1 + 30 + 20 = 61) = 130
    assert_eq!(summary.usage.total_tokens, 130);
    assert_eq!(summary.usage.input_tokens, 20);
    assert_eq!(summary.usage.output_tokens, 10);
    assert_eq!(summary.usage.cache_creation_tokens, Some(60));
    assert_eq!(summary.usage.cache_read_tokens, Some(40));
    assert_eq!(claude_inputs::load(&home, now).unwrap().0.input_tokens, 60);
    let observation = local_usage_observation(&home, "claude", now).unwrap();
    assert_eq!(observation.usage.unwrap().total_tokens, 130);
    assert!(!observation.history.is_empty());
    assert!(observation.partial_usage.is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn claude_observation_opens_each_transcript_once() {
    use std::{
        ffi::CString,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
    };
    let home = std::env::temp_dir().join(format!("widget-single-claude-{}", std::process::id()));
    let directory = home.join(".claude/projects/synthetic");
    std::fs::create_dir_all(&directory).unwrap();
    let now = Utc::now();
    let row = json!({"type":"assistant","timestamp":now.to_rfc3339(),"requestId":"req_once","message":{"role":"assistant","id":"msg_once","model":"claude-fixture","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":1}}});
    std::fs::write(directory.join("ordinary.jsonl"), row.to_string()).unwrap();
    let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(raw >= 0);
    let events = unsafe { OwnedFd::from_raw_fd(raw) };
    let path = CString::new(directory.as_os_str().as_encoded_bytes()).unwrap();
    assert!(
        unsafe {
            libc::inotify_add_watch(
                events.as_raw_fd(),
                path.as_ptr(),
                libc::IN_OPEN | libc::IN_CLOSE_NOWRITE,
            )
        } >= 0
    );
    let observation = local_usage_observation(&home, "claude", now).unwrap();
    let usage = observation.usage.unwrap();
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 1);
    assert_eq!(usage.cached_tokens, 50);
    assert_eq!(usage.total_tokens, 61);
    assert!(observation.partial_usage.is_none());
    let mut buffer = [0u8; 4096];
    let length =
        unsafe { libc::read(events.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
    assert!(length > 0);
    let mut offset = 0;
    let mut opens = 0;
    while offset < length as usize {
        let event = unsafe {
            std::ptr::read_unaligned(buffer.as_ptr().add(offset).cast::<libc::inotify_event>())
        };
        let name_start = offset + std::mem::size_of::<libc::inotify_event>();
        let name = &buffer[name_start..name_start + event.len as usize];
        if event.mask & libc::IN_OPEN != 0 && name.starts_with(b"ordinary.jsonl\0") {
            opens += 1;
        }
        offset = name_start + event.len as usize;
    }
    assert_eq!(
        opens, 1,
        "Claude transcript must only be opened once per observation"
    );
}
