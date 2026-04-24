//! LangSmith exporter for `EvalSetResult` (T149).

#![forbid(unsafe_code)]
#![cfg(feature = "langsmith")]

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::json;
use thiserror::Error;
use tokio::runtime::{Handle, RuntimeFlavor};
use url::Url;

use crate::report::{Reporter, ReporterError, ReporterOutput};
use crate::{EvalCaseResult, EvalMetricResult, EvalSetResult, Verdict};

const DEFAULT_ENDPOINT: &str = "https://api.smith.langchain.com/";

/// LangSmith reporter that pushes eval cases as runs and metric scores as feedback.
#[derive(Debug, Clone)]
pub struct LangSmithExporter {
    api_token: String,
    endpoint: Url,
}

impl LangSmithExporter {
    /// Build an exporter targeting the default LangSmith Cloud endpoint.
    #[must_use]
    pub fn new(api_token: impl Into<String>) -> Self {
        Self {
            api_token: api_token.into(),
            endpoint: Url::parse(DEFAULT_ENDPOINT).expect("default LangSmith endpoint is valid"),
        }
    }

    /// Build an exporter from `LANGSMITH_API_KEY`.
    pub fn from_env() -> Result<Self, LangSmithExportError> {
        let api_token =
            std::env::var("LANGSMITH_API_KEY").map_err(|_| LangSmithExportError::Auth)?;
        Ok(Self::new(api_token))
    }

    /// Override the LangSmith base endpoint.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: Url) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Push the entire eval-set result to LangSmith.
    pub fn export(&self, result: &EvalSetResult) -> Result<ReporterOutput, LangSmithExportError> {
        drive_async(self.export_async(result))
    }

    async fn export_async(
        &self,
        result: &EvalSetResult,
    ) -> Result<ReporterOutput, LangSmithExportError> {
        let client = reqwest::Client::new();
        let runs_url = self.api_path("runs")?;
        let feedback_url = self.api_path("feedback")?;

        for (pushed, case) in result.case_results.iter().enumerate() {
            let pushed = u32::try_from(pushed).expect("case count fits in u32");
            let run_id = self.create_run(&client, &runs_url, result, case).await?;

            for metric in &case.metric_results {
                self.create_feedback(&client, &feedback_url, &run_id, metric)
                    .await
                    .map_err(|err| LangSmithExportError::Push {
                        pushed,
                        failed: remaining_case_count(result, pushed),
                        first_error: err.to_string(),
                    })?;
            }

        }

        Ok(ReporterOutput::Remote {
            backend: "langsmith".to_string(),
            identifier: result.eval_set_id.clone(),
        })
    }

    fn api_path(&self, path: &str) -> Result<Url, LangSmithExportError> {
        let mut base = self.endpoint.clone();
        if !base.path().ends_with('/') {
            let updated = format!("{}/", base.path());
            base.set_path(&updated);
        }
        if base.path() == "/" {
            base.join(&format!("api/v1/{path}"))
        } else {
            base.join(path)
        }
        .map_err(|err| LangSmithExportError::Network(err.to_string()))
    }

    async fn create_run(
        &self,
        client: &reqwest::Client,
        url: &Url,
        result: &EvalSetResult,
        case: &EvalCaseResult,
    ) -> Result<String, LangSmithExportError> {
        let payload = json!({
            "name": format!("{}:{}", result.eval_set_id, case.case_id),
            "run_type": "chain",
            "start_time": iso_timestamp(result.timestamp),
            "end_time": iso_timestamp(result.timestamp),
            "inputs": {
                "eval_set_id": result.eval_set_id,
                "case_id": case.case_id,
                "model_provider": case.invocation.model.provider,
                "model_id": case.invocation.model.model_id,
                "turn_count": case.invocation.turns.len(),
            },
            "outputs": {
                "final_response": case.invocation.final_response,
                "verdict": verdict_name(case.verdict),
            },
            "extra": {
                "metadata": {
                    "swink_eval_set_id": result.eval_set_id,
                    "swink_case_id": case.case_id,
                    "swink_case_verdict": verdict_name(case.verdict),
                }
            }
        });

        let response = client
            .post(url.clone())
            .header("x-api-key", &self.api_token)
            .json(&payload)
            .send()
            .await
            .map_err(|err| LangSmithExportError::Network(err.to_string()))?;

        ensure_success(response).await?.json::<CreatedRun>().await.map_err(|err| {
            LangSmithExportError::Network(format!("failed to decode LangSmith run response: {err}"))
        }).map(|run| run.id)
    }

    async fn create_feedback(
        &self,
        client: &reqwest::Client,
        url: &Url,
        run_id: &str,
        metric: &EvalMetricResult,
    ) -> Result<(), LangSmithExportError> {
        let details = MetricDetails::parse(metric.details.as_deref());
        let payload = json!({
            "run_id": run_id,
            "key": details.feedback_key.unwrap_or_else(|| metric.evaluator_name.clone()),
            "score": metric.score.value,
            "comment": details.comment,
        });

        let response = client
            .post(url.clone())
            .header("x-api-key", &self.api_token)
            .json(&payload)
            .send()
            .await
            .map_err(|err| LangSmithExportError::Network(err.to_string()))?;

        ensure_success(response).await?;
        Ok(())
    }
}

impl Reporter for LangSmithExporter {
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError> {
        self.export(result).map_err(|err| match err {
            LangSmithExportError::Auth => ReporterError::Network("langsmith authentication failed".into()),
            LangSmithExportError::Network(message) => ReporterError::Network(message),
            LangSmithExportError::Push {
                pushed,
                failed,
                first_error,
            } => ReporterError::Network(format!(
                "langsmith push failed after {pushed} pushed / {failed} failed: {first_error}"
            )),
        })
    }
}

/// Structured LangSmith export failures.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LangSmithExportError {
    /// Authentication was missing or rejected.
    #[error("langsmith authentication failed")]
    Auth,
    /// Transport or serialization failure.
    #[error("langsmith network error: {0}")]
    Network(String),
    /// At least one case push failed after earlier pushes succeeded.
    #[error("langsmith push failed after {pushed} pushed / {failed} failed: {first_error}")]
    Push {
        pushed: u32,
        failed: u32,
        first_error: String,
    },
}

#[derive(Debug, Deserialize)]
struct CreatedRun {
    id: String,
}

#[derive(Debug, Default)]
struct MetricDetails {
    feedback_key: Option<String>,
    comment: Option<String>,
}

impl MetricDetails {
    fn parse(details: Option<&str>) -> Self {
        let Some(details) = details else {
            return Self::default();
        };

        let mut parsed = Self::default();
        let mut raw_lines = Vec::new();

        for line in details.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                raw_lines.push(line.to_string());
                continue;
            };

            match value.get("kind").and_then(serde_json::Value::as_str) {
                Some("feedback_key") => {
                    if let Some(key) = value.get("key").and_then(serde_json::Value::as_str) {
                        parsed.feedback_key = Some(key.to_string());
                    }
                }
                Some("note") => {
                    if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
                        raw_lines.push(text.to_string());
                    }
                }
                _ => {}
            }
        }

        if !raw_lines.is_empty() {
            parsed.comment = Some(raw_lines.join("\n"));
        }

        parsed
    }
}

async fn ensure_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, LangSmithExportError> {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(LangSmithExportError::Auth);
    }
    if status.is_success() {
        return Ok(response);
    }

    let body = response
        .text()
        .await
        .unwrap_or_else(|_| String::from("<body unavailable>"));
    Err(LangSmithExportError::Network(format!(
        "HTTP {}: {}",
        status.as_u16(),
        body
    )))
}

fn remaining_case_count(result: &EvalSetResult, pushed: u32) -> u32 {
    result.case_results.len() as u32 - pushed
}

fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "pass",
        Verdict::Fail => "fail",
    }
}

fn iso_timestamp(timestamp: u64) -> String {
    UNIX_EPOCH
        .checked_add(std::time::Duration::from_secs(timestamp))
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .map_or_else(
            |_| "1970-01-01T00:00:00Z".to_string(),
            |duration| format!("{}Z", chrono_like_seconds(duration.as_secs())),
        )
}

fn chrono_like_seconds(timestamp: u64) -> String {
    use std::fmt::Write as _;

    let datetime = time_from_unix(timestamp);
    let mut out = String::with_capacity(20);
    let _ = write!(
        out,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        datetime.year,
        datetime.month,
        datetime.day,
        datetime.hour,
        datetime.minute,
        datetime.second
    );
    out
}

struct BrokenDownTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

fn time_from_unix(timestamp: u64) -> BrokenDownTime {
    let seconds_per_day = 86_400_u64;
    let days = timestamp / seconds_per_day;
    let seconds_of_day = timestamp % seconds_per_day;

    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day % 3_600) / 60) as u32;
    let second = (seconds_of_day % 60) as u32;

    let days = i64::try_from(days).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(days);

    BrokenDownTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);

    (
        i32::try_from(year).expect("Gregorian year fits in i32"),
        u32::try_from(month).expect("month is positive"),
        u32::try_from(day).expect("day is positive"),
    )
}

fn drive_async<F, T>(future: F) -> Result<T, LangSmithExportError>
where
    F: std::future::Future<Output = Result<T, LangSmithExportError>>,
{
    if let Ok(handle) = Handle::try_current()
        && handle.runtime_flavor() == RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(|| handle.block_on(future));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| LangSmithExportError::Network(err.to_string()))?;
    runtime.block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_details_prefers_structured_feedback_key_and_note() {
        let details = concat!(
            "{\"kind\":\"prompt_version\",\"version\":\"correctness_v0\"}\n",
            "{\"kind\":\"feedback_key\",\"key\":\"quality.correctness\"}\n",
            "{\"kind\":\"note\",\"text\":\"judge note\"}"
        );

        let parsed = MetricDetails::parse(Some(details));
        assert_eq!(parsed.feedback_key.as_deref(), Some("quality.correctness"));
        assert_eq!(parsed.comment.as_deref(), Some("judge note"));
    }

    #[test]
    fn metric_details_falls_back_to_raw_text() {
        let parsed = MetricDetails::parse(Some("plain error text"));
        assert_eq!(parsed.feedback_key, None);
        assert_eq!(parsed.comment.as_deref(), Some("plain error text"));
    }
}
