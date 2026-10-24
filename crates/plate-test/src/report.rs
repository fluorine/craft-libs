//! Runs the Ghostscript plate check over scenes and writes `report.md` and `results.json`.
//!
//! Pending scenes are written as far as the writer can ([`reference::write_partial`]) and are
//! expected to fail; their results show what the writer prints today.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::judge::{self, Judgement, Verdict};
use crate::reference::{self, Context};
use crate::scene::{Level, PROCESS, Scene};
use crate::{Error, gs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Pass,
    Fail,
    /// A pending scene fails, as it should until its task lands.
    ExpectedFail,
    /// A pending scene passes: the feature works, or the scene doesn't catch its absence.
    PendingPasses,
    /// A pending scene needs a feature the writer can't leave out.
    NotWritten,
    /// The harness failed (Ghostscript, TIFF, writer error).
    Error,
}

impl Status {
    /// Whether the scene result is what the scene's `pending` field says it should be.
    pub fn as_expected(self) -> bool {
        matches!(self, Status::Pass | Status::ExpectedFail | Status::PendingPasses | Status::NotWritten)
    }

    fn label(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "**FAIL**",
            Status::ExpectedFail => "expected fail",
            Status::PendingPasses => "pending, passes",
            Status::NotWritten => "not written",
            Status::Error => "**ERROR**",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SceneResult {
    pub id: String,
    pub title: String,
    pub pending: Option<String>,
    pub status: Status,
    /// What the writer left out or skipped.
    pub notes: Vec<String>,
    /// Why the scene was not written or judged.
    pub error: Option<String>,
    pub judgement: Option<Judgement>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub ghostscript: String,
    pub resolution: f32,
    pub scenes: Vec<SceneResult>,
}

/// Write, separate and judge every scene in `ctx.scenes`.
pub fn run(ctx: &Context<'_>, ghostscript: String) -> Report {
    Report { ghostscript, resolution: gs::RESOLUTION, scenes: ctx.scenes.iter().map(|scene| check_scene(scene, ctx)).collect() }
}

/// Write, separate and judge one scene.
pub fn check_scene(scene: &Scene, ctx: &Context<'_>) -> SceneResult {
    let mut result = SceneResult {
        id: scene.id.clone(),
        title: scene.title.clone(),
        pending: scene.pending.clone(),
        status: Status::Error,
        notes: Vec::new(),
        error: None,
        judgement: None,
    };
    let written = if scene.pending.is_some() { reference::write_partial(scene, ctx) } else { reference::write(scene, ctx) };
    let output = match written {
        Ok(output) => output,
        Err(Error::Unsupported(feature)) if scene.pending.is_some() => {
            result.status = Status::NotWritten;
            result.error = Some(format!("needs {feature}"));
            return result;
        }
        Err(e) => {
            result.error = Some(e.to_string());
            return result;
        }
    };
    result.notes = output.notes;
    let plates = match gs::separate(&output.pdf, &scene.id) {
        Ok(plates) => plates,
        Err(e) => {
            result.error = Some(e.to_string());
            return result;
        }
    };
    let judgement = judge::judge(scene, &plates, gs::RESOLUTION, ctx.font.is_some());
    result.status = match (scene.pending.is_some(), judgement.passed()) {
        (false, true) => Status::Pass,
        (false, false) => Status::Fail,
        (true, false) => Status::ExpectedFail,
        (true, true) => Status::PendingPasses,
    };
    result.judgement = Some(judgement);
    result
}

/// Cargo's target directory: `CARGO_TARGET_DIR` (relative to the workspace root) or `target`.
pub fn target_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent().and_then(Path::parent).unwrap_or(manifest);
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => root.join(dir),
        None => root.join("target"),
    }
}

/// Where the report goes: `plate-test/` in the target directory.
pub fn output_dir() -> PathBuf {
    target_dir().join("plate-test")
}

/// Write `report.md` and `results.json` into `dir`; returns the report's path.
pub fn write(report: &Report, dir: &Path) -> Result<PathBuf, Error> {
    let io = |path: &Path, e: std::io::Error| Error::Scene(format!("{}: {e}", path.display()));
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let json_path = dir.join("results.json");
    let json = serde_json::to_string_pretty(report).map_err(|e| Error::Scene(e.to_string()))?;
    std::fs::write(&json_path, json).map_err(|e| io(&json_path, e))?;
    let md_path = dir.join("report.md");
    std::fs::write(&md_path, markdown(report)).map_err(|e| io(&md_path, e))?;
    Ok(md_path)
}

/// The report as Markdown.
pub fn markdown(report: &Report) -> String {
    let mut md = String::new();
    let _ = writeln!(
        md,
        "# Plate check\n\nGhostscript {}, `tiffsep` at {} dpi with overprint simulated (`/enable`). Ink % per plate \
         (C M Y K, spots by name); plates under 1% are left out. Pending scenes are written without the features \
         they wait for and are expected to fail. Generated by `scripts/plate-check.sh`.\n",
        report.ghostscript, report.resolution
    );
    md.push_str("| Scene | Status | Pending | Failing points | Plates |\n|---|---|---|---|---|\n");
    for scene in &report.scenes {
        let (points, plates) = match &scene.judgement {
            Some(j) => (format!("{}/{}", j.failed_points(), j.judged_points()), plate_problems(j).unwrap_or_else(|| "ok".into())),
            None => ("–".into(), "–".into()),
        };
        let pending = scene.pending.as_deref().unwrap_or("");
        let _ = writeln!(
            md,
            "| [{id}](#{anchor}) | {} | {pending} | {points} | {plates} |",
            scene.status.label(),
            id = scene.id,
            anchor = scene.id.to_lowercase()
        );
    }
    for scene in &report.scenes {
        let _ = writeln!(md, "\n## {}\n\n{}. Status: {}.", scene.id, scene.title, scene.status.label());
        if let Some(task) = &scene.pending {
            let _ = writeln!(md, "Pending {task}.");
        }
        if let Some(error) = &scene.error {
            let _ = writeln!(md, "\n{error}");
        }
        for note in &scene.notes {
            let _ = writeln!(md, "- {note}");
        }
        let Some(judgement) = &scene.judgement else { continue };
        let _ = writeln!(md, "\nPlates: {}.{}", judgement.plates.join(", "), plate_problems(judgement).map(|p| format!(" {p}.")).unwrap_or_default());
        md.push_str("\n| # | Point | Expected | Measured | Verdict |\n|---|---|---|---|---|\n");
        for (i, point) in judgement.points.iter().enumerate() {
            let verdict = match point.verdict {
                Verdict::Pass => "pass".to_owned(),
                Verdict::Info => "info".to_owned(),
                Verdict::Skipped => "skipped".to_owned(),
                Verdict::Fail if point.off.is_empty() => "**fail**".to_owned(),
                Verdict::Fail => format!("**fail** ({})", point.off.iter().map(|p| short(p)).collect::<Vec<_>>().join(" ")),
            };
            let how = point.how.as_deref().map(|h| format!("; {h}")).unwrap_or_default();
            let note = point.note.as_deref().map(|n| format!("{n} ")).unwrap_or_default();
            let _ = writeln!(
                md,
                "| {} | {note}({}, {}){how} | {} ±{} | {} | {verdict} |",
                i + 1,
                point.at[0],
                point.at[1],
                expected(&point.expected),
                point.tol,
                measured(&point.measured),
            );
        }
    }
    md
}

fn plate_problems(judgement: &Judgement) -> Option<String> {
    let mut parts = Vec::new();
    if !judgement.missing_plates.is_empty() {
        parts.push(format!("missing {}", judgement.missing_plates.join(", ")));
    }
    if !judgement.extra_plates.is_empty() {
        parts.push(format!("extra {}", judgement.extra_plates.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// Process plates as `C M Y K`; spots keep their names.
fn short(plate: &str) -> &str {
    match PROCESS.iter().position(|p| *p == plate) {
        Some(i) => ["C", "M", "Y", "K"].get(i).copied().unwrap_or(plate),
        None => plate,
    }
}

fn expected(inks: &std::collections::BTreeMap<String, Level>) -> String {
    let mut names: Vec<String> = inks.keys().cloned().collect();
    judge::sort_plates(&mut names);
    let parts: Vec<String> = names
        .iter()
        .filter_map(|name| match inks.get(name)? {
            Level::Value(v) if *v >= 1.0 => Some(format!("{}{v}", short(name))),
            Level::Value(_) => None,
            Level::Range([lo, hi]) => Some(format!("{}{lo}–{hi}", short(name))),
        })
        .collect();
    if parts.is_empty() { "paper".into() } else { parts.join(" ") }
}

fn measured(inks: &std::collections::BTreeMap<String, f32>) -> String {
    let mut names: Vec<String> = inks.iter().filter(|(_, v)| **v >= 1.0).map(|(k, _)| k.clone()).collect();
    judge::sort_plates(&mut names);
    let parts: Vec<String> = names.iter().filter_map(|name| Some(format!("{}{:.0}", short(name), inks.get(name)?))).collect();
    if parts.is_empty() { "paper".into() } else { parts.join(" ") }
}
