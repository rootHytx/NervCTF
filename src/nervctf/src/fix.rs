use anyhow::Result;
use dialoguer::{Confirm, Input, Select};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Default)]
struct Issues {
    missing_state: Vec<PathBuf>,
    missing_author: Vec<PathBuf>,
    missing_version: Vec<PathBuf>,
}

impl Issues {
    fn is_empty(&self) -> bool {
        self.missing_state.is_empty()
            && self.missing_author.is_empty()
            && self.missing_version.is_empty()
    }
}

/// Returns true if the YAML text has a non-commented, non-indented top-level key matching `key:`
fn has_field(contents: &str, key: &str) -> bool {
    let prefix = format!("{}:", key);
    contents.lines().any(|l| {
        // Must start at column 0 (top-level key), not inside a nested map
        !l.starts_with(' ')
            && !l.starts_with('\t')
            && !l.trim_start().starts_with('#')
            && l.starts_with(prefix.as_str())
    })
}

/// Inject `key: value` after the first line starting with `after_key:`.
/// Falls back to injecting before `fallback_key:`, then appending at end.
fn inject_field(
    contents: &str,
    key: &str,
    value: &str,
    after_key: &str,
    fallback_key: &str,
) -> String {
    let after_prefix = format!("{}:", after_key);
    let fallback_prefix = format!("{}:", fallback_key);
    let new_line = format!("{}: {}", key, value);

    let lines: Vec<&str> = contents.lines().collect();
    let mut result: Vec<String> = Vec::with_capacity(lines.len() + 1);
    let mut inserted = false;

    // Pass 1: insert after `after_key:`
    for line in &lines {
        result.push(line.to_string());
        if !inserted
            && line.trim_start().starts_with(after_prefix.as_str())
            && !line.trim_start().starts_with('#')
        {
            result.push(new_line.clone());
            inserted = true;
        }
    }

    if !inserted {
        // Pass 2: insert before `fallback_key:`
        result.clear();
        for line in &lines {
            if !inserted
                && line.trim_start().starts_with(fallback_prefix.as_str())
                && !line.trim_start().starts_with('#')
            {
                result.push(new_line.clone());
                inserted = true;
            }
            result.push(line.to_string());
        }
    }

    if !inserted {
        // Pass 3: append at end
        result.push(new_line);
    }

    let mut out = result.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn scan_issues(base_dir: &Path) -> Issues {
    let mut issues = Issues::default();

    for entry in WalkDir::new(base_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "challenge.yml")
    {
        let path = entry.path().to_path_buf();
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if !has_field(&contents, "state") {
            issues.missing_state.push(path.clone());
        }
        if !has_field(&contents, "author") {
            issues.missing_author.push(path.clone());
        }
        if !has_field(&contents, "version") {
            issues.missing_version.push(path.clone());
        }
    }

    issues
}

fn apply_fix(
    paths: &[PathBuf],
    key: &str,
    value: &str,
    after_key: &str,
    fallback_key: &str,
) -> Result<usize> {
    let mut count = 0;
    for path in paths {
        let contents = fs::read_to_string(path)?;
        let patched = inject_field(&contents, key, value, after_key, fallback_key);
        fs::write(path, patched)?;
        count += 1;
    }
    Ok(count)
}

fn print_paths(paths: &[PathBuf]) {
    for p in paths.iter().take(10) {
        println!("    {}", p.display());
    }
    if paths.len() > 10 {
        println!("    ... and {} more", paths.len() - 10);
    }
}

pub fn run_fix(base_dir: &Path, dry_run: bool) -> Result<()> {
    println!(
        "Scanning for challenge.yml issues in {} ...",
        base_dir.display()
    );
    let issues = scan_issues(base_dir);

    if issues.is_empty() {
        println!("No issues found. All challenge files look good.");
        return Ok(());
    }

    // ── state ──────────────────────────────────────────────────────────────
    if !issues.missing_state.is_empty() {
        println!(
            "\n[state]  {} file(s) missing the `state` field:",
            issues.missing_state.len()
        );
        print_paths(&issues.missing_state);

        let opts = &[
            "Set state: visible  (challenges appear to players)",
            "Set state: hidden   (challenges hidden from players)",
            "Skip",
        ];
        let sel = Select::new()
            .with_prompt("How should `state` be set?")
            .items(opts)
            .default(0)
            .interact()?;

        if sel < 2 {
            let val = if sel == 0 { "visible" } else { "hidden" };
            if dry_run {
                println!(
                    "  [dry-run] Would add `state: {}` to {} file(s)",
                    val,
                    issues.missing_state.len()
                );
            } else {
                let n = apply_fix(&issues.missing_state, "state", val, "type", "version")?;
                println!("  Added `state: {}` to {} file(s).", val, n);
            }
        } else {
            println!("  Skipped.");
        }
    }

    // ── author ─────────────────────────────────────────────────────────────
    if !issues.missing_author.is_empty() {
        println!(
            "\n[author]  {} file(s) missing the `author` field:",
            issues.missing_author.len()
        );
        print_paths(&issues.missing_author);

        let default_author: String = Input::new()
            .with_prompt("Author name to use (leave blank to skip)")
            .allow_empty(true)
            .interact_text()?;

        if default_author.trim().is_empty() {
            println!("  Skipped.");
        } else if dry_run {
            println!(
                "  [dry-run] Would add `author: {}` to {} file(s)",
                default_author,
                issues.missing_author.len()
            );
        } else {
            let n = apply_fix(
                &issues.missing_author,
                "author",
                &default_author,
                "name",
                "category",
            )?;
            println!("  Added `author: {}` to {} file(s).", default_author, n);
        }
    }

    // ── version ────────────────────────────────────────────────────────────
    if !issues.missing_version.is_empty() {
        println!(
            "\n[version]  {} file(s) missing the `version` field:",
            issues.missing_version.len()
        );
        print_paths(&issues.missing_version);

        let proceed = Confirm::new()
            .with_prompt("Add `version: '0.3'` to all?")
            .default(true)
            .interact()?;

        if proceed {
            if dry_run {
                println!(
                    "  [dry-run] Would add `version: '0.3'` to {} file(s)",
                    issues.missing_version.len()
                );
            } else {
                let n = apply_fix(
                    &issues.missing_version,
                    "version",
                    "'0.3'",
                    "files",
                    "flags",
                )?;
                println!("  Added `version: '0.3'` to {} file(s).", n);
            }
        } else {
            println!("  Skipped.");
        }
    }

    if dry_run {
        println!("\nDry run complete -- no files were modified.");
    } else {
        println!("\nFix complete.");
    }

    Ok(())
}
