use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::{verify_program, ShrinkResult};

pub fn shrink_program(path: &Path) -> Result<ShrinkResult> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read `{}`", path.display()))?;
    let report = verify_program(path)?;
    if report.divergences.is_empty() {
        bail!("`{}` does not diverge; nothing to shrink", path.display());
    }

    let mut lines: Vec<String> = original.lines().map(|line| line.to_string()).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for idx in 0..lines.len() {
            let line = lines[idx].trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut candidate = lines.clone();
            candidate.remove(idx);
            let candidate_source = candidate.join("\n");
            let tmp =
                tempfile::NamedTempFile::new().context("failed to create shrink candidate file")?;
            std::fs::write(tmp.path(), &candidate_source).context("failed to write candidate")?;
            let candidate_report = verify_program(tmp.path());
            if let Ok(candidate_report) = candidate_report {
                if !candidate_report.divergences.is_empty() {
                    lines = candidate;
                    changed = true;
                    break;
                }
            }
        }
    }

    let output = path.with_file_name(format!(
        "{}.shrunk.cor",
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("reproducer")
    ));
    std::fs::write(&output, lines.join("\n"))
        .with_context(|| format!("failed to write `{}`", output.display()))?;
    Ok(ShrinkResult {
        original: path.to_path_buf(),
        output,
        removed_lines: original.lines().count().saturating_sub(lines.len()),
    })
}
