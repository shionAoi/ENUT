//! Organization cost reporting, separate from subscription balances and device activity.
//! Credentials are explicitly supplied, retained in session memory only, and never logged.
use chrono::{Datelike, SecondsFormat, TimeZone, Utc};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Mutex,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const ERROR: &str = "Cost report unavailable. Check organization reporting permissions. Reports may be delayed; no balance or token allowance is inferred.";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingReport {
    pub provider: String,
    pub spend_usd: Option<f64>,
    pub period_start: String,
    pub period_end: String,
    pub as_of: Option<String>,
    pub message: String,
}

pub fn report_url(provider: &str) -> Result<&'static str, String> {
    match provider {
        "openai" => Ok("https://api.openai.com/v1/organization/costs"),
        "anthropic" => Ok("https://api.anthropic.com/v1/organizations/cost_report"),
        _ => Err("Unsupported reporting provider.".into()),
    }
}

pub fn parse_costs(provider: &str, value: &Value) -> Result<f64, String> {
    report_url(provider)?;
    // Never publish an incomplete page as a complete month-to-date total.
    if value["has_more"].as_bool() != Some(false) {
        return Err(ERROR.into());
    }
    let buckets = value["data"]
        .as_array()
        .filter(|a| a.len() <= 31)
        .ok_or(ERROR)?;
    let mut total = 0.0;
    for bucket in buckets {
        let rows = bucket["results"]
            .as_array()
            .filter(|a| a.len() <= 4096)
            .ok_or(ERROR)?;
        for row in rows {
            let (currency, amount) = if provider == "openai" {
                (
                    row["amount"]["currency"].as_str(),
                    row["amount"]["value"].as_f64(),
                )
            } else {
                (
                    row["currency"].as_str(),
                    row["amount"]
                        .as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                        .map(|n| n / 100.0),
                )
            };
            if !currency.is_some_and(|c| c.eq_ignore_ascii_case("usd")) {
                return Err(ERROR.into());
            }
            let amount = amount.filter(|n| n.is_finite()).ok_or(ERROR)?;
            total += amount;
        }
    }
    if total.is_finite() {
        Ok(total)
    } else {
        Err(ERROR.into())
    }
}

pub type ReportFuture = Pin<Box<dyn Future<Output = Result<BillingReport, String>> + Send>>;
pub trait ReportReader: Send + Sync {
    fn read(&self, provider: String, credential: Zeroizing<String>) -> ReportFuture;
}
pub struct ReportingClient;
impl ReportReader for ReportingClient {
    fn read(&self, provider: String, credential: Zeroizing<String>) -> ReportFuture {
        Box::pin(async move {
            let end = Utc::now();
            let start = Utc
                .with_ymd_and_hms(end.year(), end.month(), 1, 0, 0, 0)
                .single()
                .ok_or(ERROR)?;
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_secs(4))
                .build()
                .map_err(|_| ERROR)?;
            let request = client
                .get(report_url(&provider)?)
                .header("User-Agent", concat!("AIUsageMonitor/", env!("CARGO_PKG_VERSION")));
            let request = if provider == "openai" {
                request.bearer_auth(credential.as_str()).query(&[
                    ("start_time", start.timestamp().to_string()),
                    ("end_time", end.timestamp().to_string()),
                    ("bucket_width", "1d".into()),
                    ("limit", "31".into()),
                ])
            } else {
                request
                    .header("x-api-key", credential.as_str())
                    .header("anthropic-version", "2026-10-21")
                    .query(&[
                        (
                            "starting_at",
                            start.to_rfc3339_opts(SecondsFormat::Secs, true),
                        ),
                        ("ending_at", end.to_rfc3339_opts(SecondsFormat::Secs, true)),
                        ("bucket_width", "1d".into()),
                        ("limit", "31".into()),
                    ])
            };
            let mut response = request.send().await.map_err(|_| ERROR)?;
            if !response.status().is_success() {
                return Err(ERROR.into());
            }
            let mut bytes = vec![];
            while let Some(chunk) = response.chunk().await.map_err(|_| ERROR)? {
                if bytes.len() + chunk.len() > 1024 * 1024 {
                    return Err(ERROR.into());
                }
                bytes.extend_from_slice(&chunk);
            }
            let json: Value = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
            Ok(BillingReport { provider: provider.clone(), spend_usd: Some(parse_costs(&provider, &json)?),
                period_start: start.to_rfc3339_opts(SecondsFormat::Secs, true),
                period_end: end.to_rfc3339_opts(SecondsFormat::Secs, true),
                as_of: Some(crate::dto::now()),
                message: if provider == "anthropic" { "Organization month-to-date USD costs from the supported legacy Cost API. Excludes Priority Tier; reporting may be delayed. Not a subscription balance or this computer's spend." } else { "Organization month-to-date USD costs. Provider reporting may be delayed. Not a subscription balance or this computer's spend." }.into(),
            })
        })
    }
}

struct Session {
    generation: u64,
    credential: Zeroizing<String>,
    last_attempt: Option<Instant>,
    report: BillingReport,
}
#[derive(Default)]
struct State {
    generation: u64,
    sessions: HashMap<String, Session>,
}
pub struct BillingStore<R: ReportReader> {
    state: Mutex<State>,
    refresh_lock: tokio::sync::Mutex<()>,
    reader: R,
}
impl<R: ReportReader> BillingStore<R> {
    pub fn new(reader: R) -> Self {
        Self {
            state: Mutex::new(State::default()),
            refresh_lock: tokio::sync::Mutex::new(()),
            reader,
        }
    }
    pub fn connect(&self, provider: &str, credential: String) -> Result<(), String> {
        let credential = Zeroizing::new(credential);
        report_url(provider)?;
        if credential.len() < 10
            || credential.len() > 4096
            || credential.chars().any(char::is_whitespace)
        {
            return Err(
                "Enter a valid organization reporting credential without whitespace.".into(),
            );
        }
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        let generation = state.generation;
        state.sessions.insert(
            provider.into(),
            Session {
                generation,
                credential,
                last_attempt: None,
                report: BillingReport {
                    provider: provider.into(),
                    spend_usd: None,
                    period_start: String::new(),
                    period_end: String::new(),
                    as_of: None,
                    message: "Reporting permission granted for this session; waiting for provider."
                        .into(),
                },
            },
        );
        Ok(())
    }
    pub fn disconnect(&self, provider: &str) -> Result<(), String> {
        report_url(provider)?;
        self.state.lock().unwrap().sessions.remove(provider);
        Ok(())
    }
    pub fn reports(&self) -> Vec<BillingReport> {
        let mut reports: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .sessions
            .values()
            .map(|s| s.report.clone())
            .collect();
        reports.sort_by(|a, b| a.provider.cmp(&b.provider));
        reports
    }
    pub async fn refresh(&self) {
        let _refresh = self.refresh_lock.lock().await;
        let providers: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .sessions
            .keys()
            .cloned()
            .collect();
        for provider in providers {
            let work = {
                let mut state = self.state.lock().unwrap();
                state.sessions.get_mut(&provider).and_then(|s| {
                    if s.last_attempt
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(300))
                    {
                        return None;
                    }
                    s.last_attempt = Some(Instant::now());
                    Some((s.generation, s.credential.clone()))
                })
            };
            let Some((generation, credential)) = work else {
                continue;
            };
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                self.reader.read(provider.clone(), credential),
            )
            .await
            .unwrap_or_else(|_| Err(ERROR.into()));
            let mut state = self.state.lock().unwrap();
            if let Some(session) = state
                .sessions
                .get_mut(&provider)
                .filter(|s| s.generation == generation)
            {
                match result {
                    Ok(report) => session.report = report,
                    Err(_) => session.report.message = ERROR.into(),
                }
            }
        }
    }
}
