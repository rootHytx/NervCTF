//! Challenge YAML validator
//! Checks challenge structs for correctness before deploying to CTFd.

use crate::ctfd_api::models::{Challenge, ChallengeType, FlagContent};
use std::collections::{HashMap, HashSet};
use std::path::Path;

// ── Severity ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

// Errors sort before warnings
impl PartialOrd for Severity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Severity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Severity::Error, Severity::Warning) => std::cmp::Ordering::Less,
            (Severity::Warning, Severity::Error) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        }
    }
}

// ── Issue ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Issue {
    pub severity: Severity,
    /// Empty string for cross-challenge issues
    pub challenge: String,
    pub field: Option<String>,
    pub message: String,
}

impl Issue {
    fn error(challenge: &str, field: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            challenge: challenge.to_string(),
            field: Some(field.to_string()),
            message: message.into(),
        }
    }

    fn warn(challenge: &str, field: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            challenge: challenge.to_string(),
            field: Some(field.to_string()),
            message: message.into(),
        }
    }

    fn error_global(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            challenge: String::new(),
            field: None,
            message: message.into(),
        }
    }
}

// ── Report ────────────────────────────────────────────────────────────────────

pub struct ValidationReport {
    pub issues: Vec<Issue>,
}

impl ValidationReport {
    pub fn has_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .count()
    }

    pub fn print(&self) {
        if self.is_clean() {
            println!("✅ All challenges valid — no issues found.");
            return;
        }

        for issue in &self.issues {
            let prefix = match issue.severity {
                Severity::Error => "❌ ERROR  ",
                Severity::Warning => "⚠️  WARN   ",
            };
            if issue.challenge.is_empty() {
                println!("  {} {}", prefix, issue.message);
            } else if let Some(ref field) = issue.field {
                println!(
                    "  {} [{}.{}] {}",
                    prefix, issue.challenge, field, issue.message
                );
            } else {
                println!("  {} [{}] {}", prefix, issue.challenge, issue.message);
            }
        }

        println!();
        let e = self.error_count();
        let w = self.warning_count();
        if e > 0 && w > 0 {
            println!("  {} error(s), {} warning(s)", e, w);
        } else if e > 0 {
            println!("  {} error(s)", e);
        } else {
            println!("  {} warning(s)", w);
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Validate a slice of challenges and return a report.
/// Issues are sorted: errors first, then alphabetically by challenge name.
pub fn validate_challenges(challenges: &[Challenge]) -> ValidationReport {
    let mut issues = Vec::new();

    // ── Cross-challenge: duplicate names ──────────────────────────────────────
    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for c in challenges {
        *name_counts.entry(c.name.as_str()).or_insert(0) += 1;
    }
    for (name, count) in &name_counts {
        if *count > 1 {
            issues.push(Issue::error_global(format!(
                "duplicate challenge name '{}' appears {} times",
                name, count
            )));
        }
    }

    let all_names: HashSet<&str> = challenges.iter().map(|c| c.name.as_str()).collect();

    // ── Per-challenge checks ──────────────────────────────────────────────────
    for c in challenges {
        issues.extend(validate_one(c, &all_names));
    }

    // Sort: errors first, then by challenge name
    issues.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.challenge.cmp(&b.challenge))
    });

    ValidationReport { issues }
}

// ── Per-challenge validation ──────────────────────────────────────────────────

fn validate_one(c: &Challenge, all_names: &HashSet<&str>) -> Vec<Issue> {
    let mut issues = Vec::new();
    let name = c.name.as_str();

    // name
    if c.name.trim().is_empty() {
        issues.push(Issue::error(name, "name", "must not be empty"));
    }

    // category
    if c.category.trim().is_empty() {
        issues.push(Issue::error(name, "category", "must not be empty"));
    }

    // value / dynamic extra
    match c.challenge_type {
        ChallengeType::Dynamic => match &c.extra {
            None => issues.push(Issue::error(
                name,
                "extra",
                "required for dynamic challenges — must have initial, decay, minimum",
            )),
            Some(extra) => {
                match extra.initial {
                    None => issues.push(Issue::error(
                        name,
                        "extra.initial",
                        "required for dynamic challenges",
                    )),
                    Some(0) => issues.push(Issue::error(name, "extra.initial", "must be > 0")),
                    _ => {}
                }
                match extra.decay {
                    None => issues.push(Issue::error(
                        name,
                        "extra.decay",
                        "required for dynamic challenges",
                    )),
                    Some(0) => issues.push(Issue::error(name, "extra.decay", "must be > 0")),
                    _ => {}
                }
                if extra.minimum.is_none() {
                    issues.push(Issue::warn(
                        name,
                        "extra.minimum",
                        "not set (CTFd will default to 0)",
                    ));
                }
            }
        },
        ChallengeType::Standard => {
            if c.value == 0 {
                issues.push(Issue::error(name, "value", "must be > 0"));
            }
        }
    }

    // description
    match &c.description {
        None => issues.push(Issue::warn(name, "description", "missing")),
        Some(d) if d.trim().is_empty() => issues.push(Issue::warn(name, "description", "empty")),
        _ => {}
    }

    // flags
    let flags = c.flags.as_deref().unwrap_or(&[]);
    if flags.is_empty() {
        issues.push(Issue::error(name, "flags", "no flags defined"));
    } else {
        for (i, flag) in flags.iter().enumerate() {
            let content = match flag {
                FlagContent::Simple(s) => s.as_str(),
                FlagContent::Detailed { content, .. } => content.as_str(),
            };
            if content.trim().is_empty() {
                issues.push(Issue::error(
                    name,
                    &format!("flags[{}]", i),
                    "empty flag content",
                ));
            }
        }
    }

    // files — check each referenced file exists on disk
    if let Some(files) = &c.files {
        for file in files {
            let path = Path::new(&c.source_path).join(file);
            if !path.exists() {
                issues.push(Issue::error(
                    name,
                    "files",
                    format!("'{}' not found at {}", file, path.display()),
                ));
            }
        }
    }

    // requirements — warn if a named prerequisite isn't in the local challenge set
    if let Some(reqs) = &c.requirements {
        for req in reqs.prerequisite_names() {
            // Numeric IDs refer to remote CTFd challenges — skip
            if req.parse::<u32>().is_ok() {
                continue;
            }
            if req == name {
                issues.push(Issue::error(
                    name,
                    "requirements",
                    "challenge lists itself as a prerequisite",
                ));
            } else if !all_names.contains(req.as_str()) {
                issues.push(Issue::warn(
                    name,
                    "requirements",
                    format!("prerequisite '{}' not found in local challenges", req),
                ));
            }
        }
    }

    // next — warn if the target doesn't exist locally
    if let Some(next) = &c.next {
        if next == name {
            issues.push(Issue::error(name, "next", "challenge points to itself"));
        } else if !all_names.contains(next.as_str()) {
            issues.push(Issue::warn(
                name,
                "next",
                format!("'{}' not found in local challenges", next),
            ));
        }
    }

    issues
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctfd_api::models::{
        ChallengeType, Extra, FlagContent, FlagType, Requirements, State,
    };
    use std::fs;
    use tempfile::tempdir;

    // ── Helper ────────────────────────────────────────────────────────────────

    fn make_challenge(name: &str) -> Challenge {
        Challenge {
            name: name.to_string(),
            category: "web".to_string(),
            value: 100,
            challenge_type: ChallengeType::Standard,
            description: Some("A test challenge.".to_string()),
            id: None,
            challenge_id: None,
            author: None,
            extra: None,
            image: None,
            protocol: None,
            host: None,
            connection_info: None,
            healthcheck: None,
            attempts: None,
            flags: Some(vec![FlagContent::Simple("flag{test}".to_string())]),
            topics: None,
            tags: None,
            files: None,
            hints: None,
            requirements: None,
            next: None,
            state: Some(State::Visible),
            script: None,
            solved_by_me: None,
            solves: None,
            template: None,
            version: "0.1".to_string(),
            source_path: String::new(),
        }
    }

    fn errors(report: &ValidationReport) -> Vec<&Issue> {
        report
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect()
    }

    fn warnings(report: &ValidationReport) -> Vec<&Issue> {
        report
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .collect()
    }

    fn has_issue(report: &ValidationReport, field: &str, fragment: &str) -> bool {
        report.issues.iter().any(|i| {
            i.field.as_deref() == Some(field) && i.message.contains(fragment)
        })
    }

    // ── Clean challenge ───────────────────────────────────────────────────────

    #[test]
    fn clean_challenge_no_issues() {
        let c = make_challenge("test");
        let report = validate_challenges(&[c]);
        assert!(report.is_clean(), "Expected no issues, got: {:?}", report.issues);
    }

    // ── name ──────────────────────────────────────────────────────────────────

    #[test]
    fn empty_name_is_error() {
        let mut c = make_challenge("");
        c.name = String::new();
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "name", "must not be empty"));
        assert!(!errors(&report).is_empty());
    }

    #[test]
    fn whitespace_name_is_error() {
        let mut c = make_challenge("   ");
        c.name = "   ".to_string();
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "name", "must not be empty"));
    }

    // ── category ──────────────────────────────────────────────────────────────

    #[test]
    fn empty_category_is_error() {
        let mut c = make_challenge("test");
        c.category = String::new();
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "category", "must not be empty"));
    }

    // ── value ─────────────────────────────────────────────────────────────────

    #[test]
    fn zero_value_standard_is_error() {
        let mut c = make_challenge("test");
        c.value = 0;
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "value", "must be > 0"));
    }

    #[test]
    fn nonzero_value_standard_is_ok() {
        let mut c = make_challenge("test");
        c.value = 1;
        let report = validate_challenges(&[c]);
        assert!(!has_issue(&report, "value", "must be > 0"));
    }

    #[test]
    fn zero_value_dynamic_no_value_error() {
        // Dynamic challenges use extra.initial, not value — value=0 is fine
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.value = 0;
        c.extra = Some(Extra {
            initial: Some(500),
            decay: Some(50),
            minimum: Some(100),
        });
        let report = validate_challenges(&[c]);
        assert!(!has_issue(&report, "value", "must be > 0"));
    }

    // ── dynamic extra ─────────────────────────────────────────────────────────

    #[test]
    fn dynamic_missing_extra_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = None;
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra", "required for dynamic"));
    }

    #[test]
    fn dynamic_initial_zero_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(0), decay: Some(50), minimum: Some(10) });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.initial", "must be > 0"));
    }

    #[test]
    fn dynamic_initial_missing_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: None, decay: Some(50), minimum: Some(10) });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.initial", "required"));
    }

    #[test]
    fn dynamic_decay_zero_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(500), decay: Some(0), minimum: Some(10) });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.decay", "must be > 0"));
    }

    #[test]
    fn dynamic_decay_missing_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(500), decay: None, minimum: Some(10) });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.decay", "required"));
    }

    #[test]
    fn dynamic_missing_minimum_is_warning() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(500), decay: Some(50), minimum: None });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.minimum", "not set"));
        assert!(warnings(&report).iter().any(|i| i.field.as_deref() == Some("extra.minimum")));
    }

    #[test]
    fn dynamic_all_valid_no_errors() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(500), decay: Some(50), minimum: Some(100) });
        let report = validate_challenges(&[c]);
        assert!(errors(&report).is_empty());
    }

    // ── description ───────────────────────────────────────────────────────────

    #[test]
    fn missing_description_is_warning() {
        let mut c = make_challenge("test");
        c.description = None;
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "description", "missing"));
        assert!(warnings(&report).iter().any(|i| i.field.as_deref() == Some("description")));
    }

    #[test]
    fn empty_description_is_warning() {
        let mut c = make_challenge("test");
        c.description = Some(String::new());
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "description", "empty"));
    }

    #[test]
    fn present_description_no_warning() {
        let mut c = make_challenge("test");
        c.description = Some("Find the flag!".to_string());
        let report = validate_challenges(&[c]);
        assert!(!has_issue(&report, "description", "missing"));
        assert!(!has_issue(&report, "description", "empty"));
    }

    // ── flags ─────────────────────────────────────────────────────────────────

    #[test]
    fn no_flags_is_error() {
        let mut c = make_challenge("test");
        c.flags = None;
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "flags", "no flags defined"));
    }

    #[test]
    fn empty_flags_vec_is_error() {
        let mut c = make_challenge("test");
        c.flags = Some(vec![]);
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "flags", "no flags defined"));
    }

    #[test]
    fn empty_flag_content_is_error() {
        let mut c = make_challenge("test");
        c.flags = Some(vec![FlagContent::Simple(String::new())]);
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "flags[0]", "empty flag content"));
    }

    #[test]
    fn whitespace_flag_content_is_error() {
        let mut c = make_challenge("test");
        c.flags = Some(vec![FlagContent::Simple("   ".to_string())]);
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "flags[0]", "empty flag content"));
    }

    #[test]
    fn empty_detailed_flag_content_is_error() {
        let mut c = make_challenge("test");
        c.flags = Some(vec![FlagContent::Detailed {
            id: None,
            challenge_id: None,
            type_: FlagType::Static,
            content: String::new(),
            data: None,
        }]);
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "flags[0]", "empty flag content"));
    }

    #[test]
    fn multiple_flags_second_empty_reports_correct_index() {
        let mut c = make_challenge("test");
        c.flags = Some(vec![
            FlagContent::Simple("flag{ok}".to_string()),
            FlagContent::Simple(String::new()),
        ]);
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "flags[1]", "empty flag content"));
        assert!(!has_issue(&report, "flags[0]", "empty flag content"));
    }

    // ── files ─────────────────────────────────────────────────────────────────

    #[test]
    fn referenced_file_not_found_is_error() {
        let tmp = tempdir().unwrap();
        let mut c = make_challenge("test");
        c.source_path = tmp.path().to_string_lossy().to_string();
        c.files = Some(vec!["nonexistent.zip".to_string()]);
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "files", "not found"));
    }

    #[test]
    fn referenced_file_exists_no_error() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("exploit.py"), b"# exploit").unwrap();
        let mut c = make_challenge("test");
        c.source_path = tmp.path().to_string_lossy().to_string();
        c.files = Some(vec!["exploit.py".to_string()]);
        let report = validate_challenges(&[c]);
        assert!(!has_issue(&report, "files", "not found"));
    }

    #[test]
    fn multiple_files_only_missing_one_errors() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("present.py"), b"").unwrap();
        let mut c = make_challenge("test");
        c.source_path = tmp.path().to_string_lossy().to_string();
        c.files = Some(vec!["present.py".to_string(), "missing.bin".to_string()]);
        let report = validate_challenges(&[c]);
        assert!(report.issues.iter().any(|i| i.message.contains("missing.bin")));
        assert!(!report.issues.iter().any(|i| i.message.contains("present.py")));
    }

    // ── requirements ─────────────────────────────────────────────────────────

    #[test]
    fn requirement_self_reference_is_error() {
        let mut c = make_challenge("test");
        c.requirements = Some(Requirements::Simple(vec![
            serde_json::Value::String("test".to_string()),
        ]));
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "requirements", "itself"));
    }

    #[test]
    fn requirement_not_in_local_set_is_warning() {
        let mut c = make_challenge("test");
        c.requirements = Some(Requirements::Simple(vec![
            serde_json::Value::String("other-challenge".to_string()),
        ]));
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "requirements", "not found in local"));
        assert!(warnings(&report).iter().any(|i| i.field.as_deref() == Some("requirements")));
    }

    #[test]
    fn requirement_found_in_local_set_no_issue() {
        let c1 = make_challenge("alpha");
        let mut c2 = make_challenge("beta");
        c2.requirements = Some(Requirements::Simple(vec![
            serde_json::Value::String("alpha".to_string()),
        ]));
        let report = validate_challenges(&[c1, c2]);
        assert!(!has_issue(&report, "requirements", "not found in local"));
        assert!(!has_issue(&report, "requirements", "itself"));
    }

    #[test]
    fn numeric_requirement_id_is_skipped() {
        let mut c = make_challenge("test");
        // Numeric IDs refer to remote CTFd challenges — validator must skip them
        c.requirements = Some(Requirements::Simple(vec![
            serde_json::Value::Number(42.into()),
        ]));
        let report = validate_challenges(&[c]);
        assert!(!has_issue(&report, "requirements", "not found in local"));
    }

    // ── next ──────────────────────────────────────────────────────────────────

    #[test]
    fn next_self_reference_is_error() {
        let mut c = make_challenge("test");
        c.next = Some("test".to_string());
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "next", "itself"));
    }

    #[test]
    fn next_not_in_local_set_is_warning() {
        let mut c = make_challenge("test");
        c.next = Some("follow-up".to_string());
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "next", "not found in local"));
    }

    #[test]
    fn next_found_in_local_set_no_issue() {
        let c1 = make_challenge("part-1");
        let mut c2 = make_challenge("part-2");
        c2.next = Some("part-1".to_string()); // part-1 exists
        let report = validate_challenges(&[c1, c2]);
        assert!(!has_issue(&report, "next", "not found in local"));
        assert!(!has_issue(&report, "next", "itself"));
    }

    // ── duplicate names ───────────────────────────────────────────────────────

    #[test]
    fn duplicate_names_is_error() {
        let c1 = make_challenge("dup");
        let c2 = make_challenge("dup");
        let report = validate_challenges(&[c1, c2]);
        assert!(report
            .issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("dup")));
    }

    #[test]
    fn unique_names_no_duplicate_error() {
        let c1 = make_challenge("alpha");
        let c2 = make_challenge("beta");
        let report = validate_challenges(&[c1, c2]);
        assert!(!report.issues.iter().any(|i| i.message.contains("duplicate")));
    }

    // ── ValidationReport methods ──────────────────────────────────────────────

    #[test]
    fn report_has_errors_true_when_errors_present() {
        let mut c = make_challenge("test");
        c.flags = None;
        let report = validate_challenges(&[c]);
        assert!(report.has_errors());
    }

    #[test]
    fn report_has_errors_false_when_only_warnings() {
        let mut c = make_challenge("test");
        c.description = None; // warning only
        let report = validate_challenges(&[c]);
        assert!(!report.has_errors());
    }

    #[test]
    fn report_is_clean_with_valid_challenge() {
        let c = make_challenge("clean");
        let report = validate_challenges(&[c]);
        assert!(report.is_clean());
    }

    #[test]
    fn report_counts_are_correct() {
        let mut c = make_challenge("test");
        c.flags = None;         // 1 error
        c.description = None;   // 1 warning
        let report = validate_challenges(&[c]);
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.warning_count(), 1);
    }

    #[test]
    fn errors_sort_before_warnings() {
        let mut c = make_challenge("test");
        c.flags = None;
        c.description = None;
        let report = validate_challenges(&[c]);
        // First issue must be an error
        assert_eq!(report.issues[0].severity, Severity::Error);
    }

    #[test]
    fn empty_challenge_list_is_clean() {
        let report = validate_challenges(&[]);
        assert!(report.is_clean());
    }
}
