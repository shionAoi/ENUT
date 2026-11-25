use crate::{Agent, Quota, Snapshot};

pub fn close_hides_window(tray_available: bool) -> bool {
    tray_available
}

fn allowances(snapshot: &Snapshot) -> impl Iterator<Item = (&Agent, &Quota, f64)> {
    snapshot
        .agents
        .iter()
        .filter(|a| a.installed && a.monitoring_allowed && a.connected)
        .flat_map(|agent| {
            agent.quotas.iter().filter_map(move |q| {
                let remaining = q.remaining_percent.filter(|n| (0.0..=100.0).contains(n))?;
                Some((agent, q, remaining))
            })
        })
}

fn display_percent(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    format!("{rounded:.2}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

pub fn quota_rows(snapshot: &Snapshot) -> Vec<String> {
    allowances(snapshot)
        .map(|(agent, quota, remaining)| {
            let label = match quota.label.rsplit_once(" · ") {
                Some((name, "300-minute window")) => format!("{name} · 5-hour window"),
                Some((name, "10080-minute window")) => format!("{name} · weekly"),
                Some((name, raw)) => format!("{name} · {}", window_name(raw).unwrap_or(raw)),
                None => quota.label.clone(),
            };
            let percent = display_percent(remaining);
            // Native menu labels are single-line, with '&' reserved for mnemonics.
            let label = label.replace(char::is_control, " ").replace('&', "&&");
            format!("{} · {label}: {percent}% left", agent.name)
        })
        .collect()
}

pub fn tooltip(snapshot: &Snapshot) -> String {
    let count = snapshot
        .agents
        .iter()
        .filter(|agent| agent.installed)
        .count();
    let base = format!(
        "EZTYN AI MONITOR · {count} detected tool{}",
        if count == 1 { "" } else { "s" }
    );
    let remaining = allowances(snapshot)
        .filter(|(agent, quota, _)| agent.id != "antigravity" || gemini_window(&quota.label))
        .map(|(_, _, remaining)| remaining)
        .min_by(f64::total_cmp);
    match remaining {
        Some(value) => format!(
            "{base} · {}% left in tightest window",
            display_percent(value)
        ),
        None => base,
    }
}

fn gemini_window(label: &str) -> bool {
    let parts: Vec<_> = label.split(" · ").collect();
    if parts.len() != 3 {
        return false;
    }
    let group = parts[0].to_ascii_lowercase();
    let Some(suffix) = group.strip_prefix("gemini") else {
        return false;
    };
    if suffix
        .chars()
        .next()
        .is_some_and(|c| !c.is_whitespace() && !"._:-".contains(c))
    {
        return false;
    }
    window_name(parts[2]).is_some()
}

fn window_name(raw: &str) -> Option<&'static str> {
    let window: String = raw
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .collect();
    match window.as_str() {
        "weekly" | "week" | "1w" | "7d" | "7days" => Some("weekly"),
        "5h" | "5hr" | "5hrs" | "5hour" | "5hours" | "fivehour" | "fivehours" => {
            Some("5-hour window")
        }
        _ => None,
    }
}
