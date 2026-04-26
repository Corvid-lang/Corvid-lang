use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
struct RatioArchive {
    generated_at: String,
    noise_floor: NoiseFloor,
    scenarios: BTreeMap<String, ScenarioRatios>,
}

#[derive(Debug, Clone, Deserialize)]
struct NoiseFloor {
    disclosed_cv_pct: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct ScenarioRatios {
    corvid_vs_python: RatioStats,
    corvid_vs_typescript: RatioStats,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RatioStats {
    median_ratio: f64,
    ci95: [f64; 2],
    p50: f64,
    p90: f64,
    p99: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScenarioComparison {
    scenario: String,
    ratio: RatioStats,
    faster_than_competitor: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchCompareReport {
    session: String,
    generated_at: String,
    target: String,
    noise_floor_cv_pct: f64,
    scenarios: Vec<ScenarioComparison>,
}

pub fn run_compare(target: &str, session: &str, json: bool) -> Result<u8> {
    let report = build_compare_report(target, session)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_compare_report(&report));
    }
    Ok(0)
}

pub fn build_compare_report(target: &str, session: &str) -> Result<BenchCompareReport> {
    let normalized = normalize_target(target)?;
    let archive_path = PathBuf::from("benches")
        .join("results")
        .join(session)
        .join("ratios.json");
    let archive_text = std::fs::read_to_string(&archive_path)
        .with_context(|| format!("read benchmark archive `{}`", archive_path.display()))?;
    let archive: RatioArchive =
        serde_json::from_str(&archive_text).context("parse benchmark ratios.json")?;

    let mut scenarios = archive
        .scenarios
        .into_iter()
        .map(|(scenario, ratios)| {
            let ratio = match normalized.as_str() {
                "python" => ratios.corvid_vs_python,
                "typescript" => ratios.corvid_vs_typescript,
                _ => unreachable!(),
            };
            ScenarioComparison {
                scenario,
                faster_than_competitor: ratio.median_ratio < 1.0,
                ratio,
            }
        })
        .collect::<Vec<_>>();
    scenarios.sort_by(|left, right| left.scenario.cmp(&right.scenario));

    Ok(BenchCompareReport {
        session: session.to_string(),
        generated_at: archive.generated_at,
        target: normalized,
        noise_floor_cv_pct: archive.noise_floor.disclosed_cv_pct,
        scenarios,
    })
}

pub fn render_compare_report(report: &BenchCompareReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Benchmark compare for session `{}` against `{}`\n\n",
        report.session, report.target
    ));
    out.push_str(&format!(
        "Generated: {}  Noise-floor CV: {:.2}%\n\n",
        report.generated_at, report.noise_floor_cv_pct
    ));
    out.push_str("Interpretation: ratios below 1.0 mean Corvid's measured orchestration overhead is lower than the comparison stack. Ratios above 1.0 mean Corvid is slower on that session.\n\n");
    for scenario in &report.scenarios {
        out.push_str(&format!(
            "- {}: median {:.3}x (95% CI {:.3}x..{:.3}x), p90 {:.3}x, p99 {:.3}x -> {}\n",
            scenario.scenario,
            scenario.ratio.median_ratio,
            scenario.ratio.ci95[0],
            scenario.ratio.ci95[1],
            scenario.ratio.p90,
            scenario.ratio.p99,
            if scenario.faster_than_competitor {
                "Corvid faster"
            } else {
                "Corvid slower"
            }
        ));
    }
    out
}

fn normalize_target(target: &str) -> Result<String> {
    match target.to_ascii_lowercase().as_str() {
        "python" => Ok("python".to_string()),
        "js" | "javascript" | "typescript" | "ts" => Ok("typescript".to_string()),
        other => Err(anyhow!(
            "unsupported benchmark target `{other}`; expected `python` or `js`"
        )),
    }
}
