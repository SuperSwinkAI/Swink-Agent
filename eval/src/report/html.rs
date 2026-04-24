//! Self-contained HTML reporter (feature `html-report`, spec 043 §FR-041).
//!
//! The HTML artifact is a single file with inline CSS and no external assets.
//! Case detail is presented via native `<details>` / `<summary>` elements, so
//! the report remains useful even with JavaScript disabled.

use std::path::PathBuf;

use askama::Template;

use crate::{EvalMetricResult, EvalSetResult, Verdict};

use super::{Reporter, ReporterError, ReporterOutput};

/// Default artifact filename for HTML reports.
pub const DEFAULT_ARTIFACT_NAME: &str = "eval-report.html";

/// Self-contained HTML reporter (spec 043 §FR-041).
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlReporter;

impl HtmlReporter {
    /// Create a new reporter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Reporter for HtmlReporter {
    fn render(&self, result: &EvalSetResult) -> Result<ReporterOutput, ReporterError> {
        let view = HtmlReportView::from(result);
        let html = view
            .render()
            .map_err(|err| ReporterError::Format(err.to_string()))?;
        Ok(ReporterOutput::Artifact {
            path: PathBuf::from(DEFAULT_ARTIFACT_NAME),
            bytes: html.into_bytes(),
        })
    }
}

#[derive(Template)]
#[template(
    ext = "html",
    escape = "html",
    source = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{{ eval_set_id }} report</title>
  <style>
    :root {
      color-scheme: light;
      --bg: #f6f1e8;
      --panel: #fffaf1;
      --ink: #1f1a17;
      --muted: #6c5f55;
      --line: #d7cabd;
      --pass: #245d43;
      --fail: #8b2e24;
      --accent: #a86f2c;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      padding: 24px;
      background:
        radial-gradient(circle at top left, rgba(168, 111, 44, 0.12), transparent 28rem),
        linear-gradient(180deg, #fbf7f0 0%, var(--bg) 100%);
      color: var(--ink);
      font: 16px/1.5 Georgia, "Times New Roman", serif;
    }
    main {
      max-width: 1080px;
      margin: 0 auto;
    }
    .hero, .summary, .case {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 18px;
      box-shadow: 0 10px 30px rgba(52, 37, 22, 0.06);
    }
    .hero {
      padding: 24px;
      margin-bottom: 18px;
    }
    .eyebrow {
      margin: 0 0 8px;
      color: var(--accent);
      font-size: 0.8rem;
      letter-spacing: 0.12em;
      text-transform: uppercase;
    }
    h1 {
      margin: 0;
      font-size: clamp(2rem, 4vw, 3rem);
      line-height: 1.05;
    }
    .subtitle {
      margin: 10px 0 0;
      color: var(--muted);
    }
    .summary {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
      gap: 12px;
      padding: 18px;
      margin-bottom: 18px;
    }
    .summary-card {
      padding: 12px 14px;
      border-radius: 14px;
      background: rgba(255, 255, 255, 0.65);
      border: 1px solid rgba(215, 202, 189, 0.8);
    }
    .summary-card dt {
      margin: 0 0 4px;
      color: var(--muted);
      font-size: 0.8rem;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }
    .summary-card dd {
      margin: 0;
      font-size: 1.2rem;
      font-weight: 700;
    }
    .cases {
      display: grid;
      gap: 12px;
    }
    .case {
      padding: 0;
      overflow: hidden;
    }
    .case details {
      width: 100%;
    }
    .case summary {
      list-style: none;
      cursor: pointer;
      padding: 16px 18px;
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto auto;
      gap: 12px;
      align-items: center;
    }
    .case summary::-webkit-details-marker { display: none; }
    .case-id {
      font-size: 1.1rem;
      font-weight: 700;
      overflow-wrap: anywhere;
    }
    .pill {
      border-radius: 999px;
      padding: 4px 10px;
      font-size: 0.82rem;
      font-weight: 700;
      letter-spacing: 0.04em;
      text-transform: uppercase;
    }
    .pass {
      color: var(--pass);
      background: rgba(36, 93, 67, 0.12);
    }
    .fail {
      color: var(--fail);
      background: rgba(139, 46, 36, 0.12);
    }
    .duration {
      color: var(--muted);
      white-space: nowrap;
    }
    .case-body {
      padding: 0 18px 18px;
      border-top: 1px solid var(--line);
    }
    table {
      width: 100%;
      border-collapse: collapse;
      margin-top: 12px;
      font-size: 0.95rem;
    }
    th, td {
      text-align: left;
      padding: 10px 8px;
      border-bottom: 1px solid var(--line);
      vertical-align: top;
    }
    th {
      color: var(--muted);
      font-size: 0.8rem;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }
    .reason {
      color: var(--muted);
      overflow-wrap: anywhere;
    }
    .empty {
      padding: 24px;
      text-align: center;
      color: var(--muted);
      border: 1px dashed var(--line);
      border-radius: 18px;
      background: rgba(255, 250, 241, 0.7);
    }
    @media (max-width: 700px) {
      body { padding: 14px; }
      .case summary {
        grid-template-columns: 1fr;
        align-items: start;
      }
    }
  </style>
</head>
<body>
  <main>
    <section class="hero">
      <p class="eyebrow">Eval Report</p>
      <h1>{{ eval_set_id }}</h1>
      <p class="subtitle">Generated at unix timestamp {{ timestamp }}. {{ passed }} / {{ total_cases }} cases passed.</p>
    </section>

    <section class="summary" aria-label="summary">
      <dl class="summary-card"><dt>Passed</dt><dd>{{ passed }} / {{ total_cases }}</dd></dl>
      <dl class="summary-card"><dt>Failed</dt><dd>{{ failed }}</dd></dl>
      <dl class="summary-card"><dt>Total Cost</dt><dd>${{ total_cost }}</dd></dl>
      <dl class="summary-card"><dt>Total Duration</dt><dd>{{ total_duration_ms }}ms</dd></dl>
      <dl class="summary-card"><dt>Total Tokens</dt><dd>{{ total_tokens }}</dd></dl>
    </section>

    {% if cases.len() == 0 %}
    <section class="empty">No case results were recorded.</section>
    {% else %}
    <section class="cases" aria-label="cases">
      {% for case in cases %}
      <article class="case">
        <details{% if loop.index0 == 0 %} open{% endif %}>
          <summary>
            <span class="case-id">{{ case.case_id }}</span>
            <span class="pill {{ case.verdict_class }}">{{ case.verdict_label }}</span>
            <span class="duration">{{ case.duration_ms }}ms</span>
          </summary>
          <div class="case-body">
            {% if case.metrics.len() == 0 %}
            <p class="reason">No evaluator metrics were recorded for this case.</p>
            {% else %}
            <table>
              <thead>
                <tr>
                  <th>Evaluator</th>
                  <th>Score</th>
                  <th>Threshold</th>
                  <th>Verdict</th>
                  <th>Reason</th>
                </tr>
              </thead>
              <tbody>
                {% for metric in case.metrics %}
                <tr>
                  <td>{{ metric.evaluator }}</td>
                  <td>{{ metric.score }}</td>
                  <td>{{ metric.threshold }}</td>
                  <td><span class="pill {{ metric.verdict_class }}">{{ metric.verdict_label }}</span></td>
                  <td class="reason">{{ metric.reason }}</td>
                </tr>
                {% endfor %}
              </tbody>
            </table>
            {% endif %}
          </div>
        </details>
      </article>
      {% endfor %}
    </section>
    {% endif %}
  </main>
</body>
</html>
"#
)]
struct HtmlReportView {
    eval_set_id: String,
    timestamp: u64,
    total_cases: usize,
    passed: usize,
    failed: usize,
    total_cost: String,
    total_duration_ms: u128,
    total_tokens: u64,
    cases: Vec<HtmlCaseView>,
}

struct HtmlCaseView {
    case_id: String,
    verdict_label: &'static str,
    verdict_class: &'static str,
    duration_ms: u128,
    metrics: Vec<HtmlMetricView>,
}

struct HtmlMetricView {
    evaluator: String,
    score: String,
    threshold: String,
    verdict_label: &'static str,
    verdict_class: &'static str,
    reason: String,
}

impl From<&EvalSetResult> for HtmlReportView {
    fn from(result: &EvalSetResult) -> Self {
        Self {
            eval_set_id: result.eval_set_id.clone(),
            timestamp: result.timestamp,
            total_cases: result.summary.total_cases,
            passed: result.summary.passed,
            failed: result.summary.failed,
            total_cost: format!("{:.6}", result.summary.total_cost.total),
            total_duration_ms: result.summary.total_duration.as_millis(),
            total_tokens: result.summary.total_usage.total,
            cases: result.case_results.iter().map(HtmlCaseView::from).collect(),
        }
    }
}

impl From<&crate::EvalCaseResult> for HtmlCaseView {
    fn from(case: &crate::EvalCaseResult) -> Self {
        Self {
            case_id: case.case_id.clone(),
            verdict_label: verdict_label(case.verdict),
            verdict_class: verdict_class(case.verdict),
            duration_ms: case.invocation.total_duration.as_millis(),
            metrics: case
                .metric_results
                .iter()
                .map(HtmlMetricView::from)
                .collect(),
        }
    }
}

impl From<&EvalMetricResult> for HtmlMetricView {
    fn from(metric: &EvalMetricResult) -> Self {
        let verdict = metric.score.verdict();
        Self {
            evaluator: metric.evaluator_name.clone(),
            score: format!("{:.2}", metric.score.value),
            threshold: format!("{:.2}", metric.score.threshold),
            verdict_label: verdict_label(verdict),
            verdict_class: verdict_class(verdict),
            reason: metric.details.clone().unwrap_or_default(),
        }
    }
}

const fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    }
}

const fn verdict_class(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "pass",
        Verdict::Fail => "fail",
    }
}
