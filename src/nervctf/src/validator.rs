//! Challenge YAML validator
//! Checks challenge structs for correctness before deploying to CTFd.

use crate::ctfd_api::models::{Challenge, ChallengeType, FlagContent, FlagType, InstanceFlagMode, State, Tag};
use crate::directory_scanner::ScanFailure;
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

    /// Print validation results.
    ///
    /// Normal mode (verbose=false): only shows parse failures and challenges
    /// with issues (compact one-liner per issue).  Clean challenges are silent.
    ///
    /// Debug mode (verbose=true): full per-field dictionary for every challenge.
    pub fn print(&self, challenges: &[Challenge], failures: &[ScanFailure], verbose: bool) {
        // ── Group issues by challenge name ────────────────────────────────────
        let mut by_challenge: HashMap<&str, Vec<&Issue>> = HashMap::new();
        let mut global: Vec<&Issue> = Vec::new();

        for issue in &self.issues {
            if issue.challenge.is_empty() {
                global.push(issue);
            } else {
                by_challenge
                    .entry(issue.challenge.as_str())
                    .or_default()
                    .push(issue);
            }
        }

        // ── Parse failures (always shown) ─────────────────────────────────────
        if !failures.is_empty() {
            println!("PARSE FAILURES  ({} file(s) could not be loaded)", failures.len());
            for f in failures {
                println!("  [x]  {}", f.path.display());
                // Indent each line of the error message
                for line in f.error.lines() {
                    println!("      {}", line);
                }
            }
            println!();
        }

        // ── Global issues (e.g. duplicate names) ─────────────────────────────
        if !global.is_empty() {
            println!("GLOBAL ISSUES");
            for i in &global {
                let tag = if i.severity == Severity::Error { "[E]" } else { "[W]" };
                println!("  {}  {}", tag, i.message);
            }
            println!();
        }

        // ── Per-challenge views ───────────────────────────────────────────────
        for c in challenges {
            let issues = by_challenge
                .get(c.name.as_str())
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if verbose {
                print_challenge_dict(c, issues);
            } else {
                print_challenge_compact(c, issues);
            }
        }

        // ── Overall summary ───────────────────────────────────────────────────
        let valid = challenges.len();
        let failed = failures.len();
        let total = valid + failed;
        let problems = by_challenge.len();
        let clean = valid.saturating_sub(problems);

        if failed == 0 && self.is_clean() {
            println!("all {} challenge(s) valid -- no issues found.", total);
        } else {
            let mut parts: Vec<String> = Vec::new();
            if clean > 0 {
                parts.push(format!("{} clean", clean));
            }
            if problems > 0 {
                parts.push(format!(
                    "{} with issues ({} error(s), {} warning(s))",
                    problems,
                    self.error_count(),
                    self.warning_count()
                ));
            }
            if failed > 0 {
                parts.push(format!("{} failed to parse", failed));
            }
            println!("Validated {} challenge(s): {}", total, parts.join(", "));
            if verbose {
                println!("\nRun `nervctf validate` (without --debug) for a compact summary.");
            } else {
                println!("\nRun `nervctf validate --debug` for the full field-by-field view.");
            }
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

    // value / dynamic / instance extra
    match c.challenge_type {
        ChallengeType::Dynamic => match &c.extra {
            None => issues.push(Issue::error(
                name,
                "extra",
                "required — must have at least initial, decay, minimum",
            )),
            Some(extra) => {
                match extra.initial {
                    None => issues.push(Issue::error(name, "extra.initial", "required")),
                    Some(0) => issues.push(Issue::error(name, "extra.initial", "must be > 0")),
                    _ => {}
                }
                match extra.decay {
                    None => issues.push(Issue::error(name, "extra.decay", "required")),
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
        ChallengeType::Instance => {
            match &c.instance {
                None => issues.push(Issue::error(
                    name,
                    "instance",
                    "required for instance challenges — add an `instance:` block with backend, internal_port, connection (or put them under `extra:`)",
                )),
                Some(inst) => {
                    if inst.internal_port == 0 {
                        issues.push(Issue::error(name, "instance.internal_port", "must be > 0"));
                    }
                    if inst.connection.trim().is_empty() {
                        issues.push(Issue::error(name, "instance.connection", "required (e.g. 'nc', 'http', 'ssh')"));
                    }
                    // Backend-specific required fields
                    match inst.backend {
                        crate::ctfd_api::models::InstanceBackend::Docker => {
                            if inst.image.as_deref().unwrap_or("").trim().is_empty() {
                                issues.push(Issue::error(
                                    name, "instance.image",
                                    "required for docker backend — docker image name or build path",
                                ));
                            }
                        }
                        crate::ctfd_api::models::InstanceBackend::Lxc => {
                            if inst.lxc_image.as_deref().unwrap_or("").trim().is_empty() {
                                issues.push(Issue::error(
                                    name, "instance.lxc_image",
                                    "required for lxc backend — LXC image alias (e.g. 'ubuntu:22.04')",
                                ));
                            }
                        }
                        crate::ctfd_api::models::InstanceBackend::Vagrant => {
                            if inst.vagrantfile.as_deref().unwrap_or("").trim().is_empty() {
                                issues.push(Issue::error(
                                    name, "instance.vagrantfile",
                                    "required for vagrant backend — path to Vagrantfile",
                                ));
                            }
                        }
                        crate::ctfd_api::models::InstanceBackend::Compose => {
                            // compose_file defaults to docker-compose.yml; no strict requirement
                        }
                    }
                    // Flag requirements
                    let is_random = matches!(inst.flag_mode, Some(InstanceFlagMode::Random));
                    if !is_random && c.flags.as_ref().map(|f| f.is_empty()).unwrap_or(true) {
                        issues.push(Issue::error(
                            name, "flags",
                            "required for instance challenges unless flag_mode: random",
                        ));
                    }
                    // flag_delivery: file requires flag_file_path
                    if matches!(inst.flag_delivery, Some(crate::ctfd_api::models::FlagDelivery::File))
                        && inst.flag_file_path.is_none()
                    {
                        issues.push(Issue::error(
                            name,
                            "instance.flag_file_path",
                            "required when flag_delivery: file — absolute path inside the container",
                        ));
                    }
                }
            }
            // Instance may optionally have extra for dynamic scoring
            if let Some(extra) = &c.extra {
                if let Some(0) = extra.initial {
                    issues.push(Issue::error(name, "extra.initial", "must be > 0"));
                }
                if let Some(0) = extra.decay {
                    issues.push(Issue::error(name, "extra.decay", "must be > 0"));
                }
            }
        }
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

    // Instance challenges with flag_mode=random auto-generate flags at runtime,
    // so `flags:` is not required for those.
    let is_random_instance = c.challenge_type == ChallengeType::Instance
        && matches!(
            c.instance.as_ref().and_then(|i| i.flag_mode.as_ref()),
            Some(InstanceFlagMode::Random)
        );

    let flags = c.flags.as_deref().unwrap_or(&[]);
    // For Instance type, flags are validated in the Instance match arm above.
    // For Standard/Dynamic, require flags unless random instance.
    let skip_flags_check = is_random_instance || c.challenge_type == ChallengeType::Instance;
    if !skip_flags_check && flags.is_empty() {
        issues.push(Issue::error(name, "flags", "no flags defined"));
    } else if !skip_flags_check {
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

    // unknown YAML keys
    for key in &c.unknown_yaml_keys {
        issues.push(Issue::warn(
            name,
            key,
            format!("unknown YAML key '{}' — not in the ctfcli spec, will be ignored", key),
        ));
    }

    issues
}

// ── Display helpers ───────────────────────────────────────────────────────────

/// Print a full dictionary view for one challenge with all fields and inline
/// issue annotations.  Challenges with no issues show only a ✅ header line.
fn print_challenge_dict(c: &Challenge, issues: &[&Issue]) {
    const FW: usize = 16; // field-name column width

    // ── Build field → issues index ────────────────────────────────────────────
    let mut by_field: HashMap<&str, Vec<&Issue>> = HashMap::new();
    for issue in issues {
        if let Some(f) = issue.field.as_deref() {
            by_field.entry(f).or_default().push(*issue);
        }
    }

    let fi = |field: &str| -> &[&Issue] {
        by_field.get(field).map(|v| v.as_slice()).unwrap_or(&[])
    };

    // ── Header ────────────────────────────────────────────────────────────────
    let has_err = issues.iter().any(|i| i.severity == Severity::Error);
    let has_warn = issues.iter().any(|i| i.severity == Severity::Warning);
    let hdr = if has_err { "[x]" } else if has_warn { "[!]" } else { "[ok]" };
    println!("{} \"{}\"  [{}]", hdr, c.name, c.category);

    if issues.is_empty() {
        // Compact view for clean challenges
        println!();
        return;
    }

    // ── Field rows ────────────────────────────────────────────────────────────
    frow(FW, "name", Some(c.name.as_str()), fi("name"));
    frow(FW, "author", c.author.as_deref(), fi("author"));
    frow(FW, "category", Some(c.category.as_str()), fi("category"));

    let desc_val = c.description.as_deref().map(|d| {
        let s = d.replace('\n', " ");
        truncate_str(&s, 70)
    });
    frow(FW, "description", desc_val.as_deref(), fi("description"));

    let type_str = match c.challenge_type {
        ChallengeType::Standard => "standard",
        ChallengeType::Dynamic => "dynamic",
        ChallengeType::Instance => "instance",
    };
    frow(FW, "type", Some(type_str), fi("type"));

    // value / extra — only the fields that are relevant per type
    match c.challenge_type {
        ChallengeType::Standard => {
            let v = c.value.to_string();
            frow(FW, "value", Some(&v), fi("value"));
        }
        ChallengeType::Dynamic => {
            if let Some(e) = &c.extra {
                let i_s = e.initial.map(|x| x.to_string());
                let d_s = e.decay.map(|x| x.to_string());
                let m_s = e.minimum.map(|x| x.to_string());
                frow(FW, "extra.initial", i_s.as_deref(), fi("extra.initial"));
                frow(FW, "extra.decay", d_s.as_deref(), fi("extra.decay"));
                frow(FW, "extra.minimum", m_s.as_deref(), fi("extra.minimum"));
            } else {
                frow(FW, "extra", None, fi("extra"));
            }
        }
        ChallengeType::Instance => {
            if let Some(inst) = &c.instance {
                let port_s = inst.internal_port.to_string();
                frow(FW, "instance.backend", Some(&format!("{:?}", inst.backend).to_lowercase()), fi("instance.backend"));
                frow(FW, "instance.internal_port", Some(&port_s), fi("instance.internal_port"));
                frow(FW, "instance.connection", Some(&inst.connection), fi("instance.connection"));
                if let Some(img) = &inst.image {
                    frow(FW, "instance.image", Some(img.as_str()), fi("instance.image"));
                }
                let fm_s = inst.flag_mode.as_ref().map(|m| match m {
                    InstanceFlagMode::Static => "static",
                    InstanceFlagMode::Random => "random",
                });
                frow(FW, "instance.flag_mode", fm_s, fi("instance.flag_mode"));
            } else {
                frow(FW, "instance", None, fi("instance"));
            }
            // Optional dynamic scoring for instance challenges
            if let Some(e) = &c.extra {
                let i_s = e.initial.map(|x| x.to_string());
                let d_s = e.decay.map(|x| x.to_string());
                let m_s = e.minimum.map(|x| x.to_string());
                frow(FW, "extra.initial", i_s.as_deref(), fi("extra.initial"));
                frow(FW, "extra.decay", d_s.as_deref(), fi("extra.decay"));
                frow(FW, "extra.minimum", m_s.as_deref(), fi("extra.minimum"));
            }
        }
    }

    // flags
    let flags_val = c.flags.as_ref().and_then(|f| {
        if f.is_empty() {
            None
        } else {
            let summaries: Vec<String> = f.iter().take(3).map(flag_display).collect();
            let tail = if f.len() > 3 {
                format!(", +{} more", f.len() - 3)
            } else {
                String::new()
            };
            Some(format!("{} flag(s): {}{}", f.len(), summaries.join(", "), tail))
        }
    });
    frow(FW, "flags", flags_val.as_deref(), fi("flags"));
    // per-flag sub-issues (flags[0], flags[1], …)
    if let Some(flags) = &c.flags {
        for (i, fc) in flags.iter().enumerate() {
            let key = format!("flags[{}]", i);
            let sub = by_field.get(key.as_str()).map(|v| v.as_slice()).unwrap_or(&[]);
            if !sub.is_empty() {
                let fval = flag_display(fc);
                frow_sub(FW, &key, Some(&fval), sub);
            }
        }
    }

    // tags
    let tags_val = c.tags.as_ref().and_then(|t| {
        if t.is_empty() {
            None
        } else {
            let names: Vec<&str> = t.iter().map(Tag::value_str).collect();
            Some(format!("[{}]", names.join(", ")))
        }
    });
    frow(FW, "tags", tags_val.as_deref(), fi("tags"));

    // topics
    let topics_val = c.topics.as_ref().and_then(|t| {
        if t.is_empty() {
            None
        } else {
            Some(format!("[{}]", t.join(", ")))
        }
    });
    frow(FW, "topics", topics_val.as_deref(), fi("topics"));

    // files
    let files_val = c.files.as_ref().and_then(|f| {
        if f.is_empty() {
            None
        } else {
            Some(format!("[{}]", f.join(", ")))
        }
    });
    frow(FW, "files", files_val.as_deref(), fi("files"));

    // hints
    let hints_val = c.hints.as_ref().and_then(|h| {
        if h.is_empty() {
            None
        } else {
            Some(format!("{} hint(s)", h.len()))
        }
    });
    frow(FW, "hints", hints_val.as_deref(), fi("hints"));

    // requirements
    let reqs_val = c.requirements.as_ref().map(|r| {
        let names = r.prerequisite_names();
        let strs: Vec<String> = names.into_iter().filter_map(|n| Some(n)).collect();
        format!("[{}]", strs.join(", "))
    });
    frow(FW, "requirements", reqs_val.as_deref(), fi("requirements"));

    // next
    frow(FW, "next", c.next.as_deref(), fi("next"));

    // state
    let state_val = match &c.state {
        None => "(defaults to visible)",
        Some(State::Visible) => "visible",
        Some(State::Hidden) => "hidden",
    };
    frow(FW, "state", Some(state_val), fi("state"));

    // connection_info
    frow(
        FW,
        "connection_info",
        c.connection_info.as_deref(),
        fi("connection_info"),
    );

    // attempts
    let att_val = c.attempts.map(|a| a.to_string());
    frow(FW, "attempts", att_val.as_deref(), fi("attempts"));

    // docker fields — only shown when set
    if let Some(img) = &c.image {
        frow(FW, "image", Some(img.as_str()), fi("image"));
    }
    if let Some(proto) = &c.protocol {
        frow(FW, "protocol", Some(proto.as_str()), fi("protocol"));
    }
    if let Some(host) = &c.host {
        frow(FW, "host", Some(host.as_str()), fi("host"));
    }
    if let Some(hc) = &c.healthcheck {
        frow(FW, "healthcheck", Some(hc.as_str()), fi("healthcheck"));
    }

    // version
    frow(FW, "version", Some(&c.version), fi("version"));

    // unknown YAML keys
    for key in &c.unknown_yaml_keys {
        let sub = by_field.get(key.as_str()).map(|v| v.as_slice()).unwrap_or(&[]);
        frow(FW, key, Some("(present in YAML)"), sub);
    }

    // Catch any issue fields not already rendered above
    const RENDERED: &[&str] = &[
        "name", "author", "category", "description", "type", "value",
        "extra", "extra.initial", "extra.decay", "extra.minimum",
        // instance fields
        "instance", "instance.backend", "instance.internal_port", "instance.connection",
        "instance.image", "instance.flag_mode",
        "instance.flag_delivery", "instance.flag_file_path", "instance.flag_service",
        "instance.flag_prefix", "instance.flag_suffix", "instance.random_flag_length",
        "instance.compose_file", "instance.compose_service",
        "instance.lxc_image", "instance.vagrantfile",
        "instance.timeout_minutes", "instance.max_per_team", "instance.max_renewals",
        "instance.command",
        "flags", "tags", "topics", "files", "hints", "requirements",
        "next", "state", "connection_info", "attempts", "image",
        "protocol", "host", "healthcheck", "version",
    ];
    for (field, field_issues) in &by_field {
        if RENDERED.contains(field) { continue; }
        if field.starts_with("flags[") || field.starts_with("files[") { continue; }
        if c.unknown_yaml_keys.iter().any(|k| k == *field) { continue; }
        frow(FW, field, None, field_issues.as_slice());
    }

    // ── Footer ────────────────────────────────────────────────────────────────
    let e = issues.iter().filter(|i| i.severity == Severity::Error).count();
    let w = issues.iter().filter(|i| i.severity == Severity::Warning).count();
    println!("  {}", "-".repeat(FW + 12));
    if e > 0 && w > 0 {
        println!("  [x] {} error(s), {} warning(s)", e, w);
    } else if e > 0 {
        println!("  [x] {} error(s)", e);
    } else {
        println!("  [!] {} warning(s)", w);
    }
    println!();
}

/// Compact view for one challenge: header + one line per issue.
/// Clean challenges produce no output (silent pass).
fn print_challenge_compact(c: &Challenge, issues: &[&Issue]) {
    if issues.is_empty() {
        return;
    }
    let has_err = issues.iter().any(|i| i.severity == Severity::Error);
    let hdr = if has_err { "[x]" } else { "[!]" };
    println!("{} \"{}\"  [{}]", hdr, c.name, c.category);
    for issue in issues {
        let tag = if issue.severity == Severity::Error { "[E]" } else { "[W]" };
        let field = issue.field.as_deref().unwrap_or("(global)");
        println!("   {}  {}: {}", tag, field, issue.message);
    }
    println!();
}

/// Print one field row with status tag and optional inline issue messages.
///
/// Status tags (3 ASCII chars, always same display width):
///   `[E]`  — field has at least one error
///   `[W]`  — field has at least one warning
///   ` ok`  — field is set, no issues
///   ` --`  — field not set, no issues
fn frow(fw: usize, field: &str, value: Option<&str>, issues: &[&Issue]) {
    let has_e = issues.iter().any(|i| i.severity == Severity::Error);
    let has_w = issues.iter().any(|i| i.severity == Severity::Warning);

    let (tag, val_str): (&str, String) = if has_e {
        ("[E]", value.unwrap_or("MISSING").to_owned())
    } else if has_w {
        ("[W]", value.unwrap_or("(not set)").to_owned())
    } else if let Some(v) = value {
        (" ok", v.to_owned())
    } else {
        (" --", "(not set)".to_owned())
    };

    if issues.is_empty() {
        println!("  {:<fw$}  {}  {}", field, tag, val_str, fw = fw);
    } else {
        let msgs = issues
            .iter()
            .map(|i| i.message.as_str())
            .collect::<Vec<_>>()
            .join(";  ");
        println!("  {:<fw$}  {}  {}  ->  {}", field, tag, val_str, msgs, fw = fw);
    }
}

/// Like `frow` but indented one level for sub-field issues (e.g. flags[0]).
fn frow_sub(fw: usize, field: &str, value: Option<&str>, issues: &[&Issue]) {
    let has_e = issues.iter().any(|i| i.severity == Severity::Error);
    let has_w = issues.iter().any(|i| i.severity == Severity::Warning);
    let tag = if has_e { "[E]" } else if has_w { "[W]" } else { "   " };
    let val_str = value.unwrap_or("(empty)");
    let msgs = issues
        .iter()
        .map(|i| i.message.as_str())
        .collect::<Vec<_>>()
        .join(";  ");
    println!("    {:<fw$}  {}  {}  ->  {}", field, tag, val_str, msgs, fw = fw.saturating_sub(2));
}

/// Summarise one flag for display (truncated content with type prefix).
fn flag_display(fc: &FlagContent) -> String {
    match fc {
        FlagContent::Simple(s) => {
            if s.is_empty() {
                "(empty)".to_owned()
            } else {
                truncate_str(s, 40)
            }
        }
        FlagContent::Detailed { content, type_, .. } => {
            let prefix = match type_ {
                FlagType::Static => "static",
                FlagType::Regex => "regex",
            };
            if content.is_empty() {
                format!("{}: (empty)", prefix)
            } else {
                format!("{}: {}", prefix, truncate_str(content, 34))
            }
        }
    }
}

/// Truncate a string to at most `max_chars` characters, appending "…" if cut.
fn truncate_str(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let mut result = String::with_capacity(max_chars);
    for _ in 0..max_chars.saturating_sub(1) {
        match chars.next() {
            Some(c) => result.push(c),
            None => return result,
        }
    }
    // check if more chars remain
    if chars.next().is_some() {
        result.push_str("...");
    } else if let Some(last) = s.chars().last() {
        result.push(last);
    }
    result
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
            instance: None,
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
            unknown_yaml_keys: Vec::new(),
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
            ..Default::default()
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
        assert!(has_issue(&report, "extra", "required"));
    }

    #[test]
    fn dynamic_initial_zero_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(0), decay: Some(50), minimum: Some(10), ..Default::default() });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.initial", "must be > 0"));
    }

    #[test]
    fn dynamic_initial_missing_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: None, decay: Some(50), minimum: Some(10), ..Default::default() });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.initial", "required"));
    }

    #[test]
    fn dynamic_decay_zero_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(500), decay: Some(0), minimum: Some(10), ..Default::default() });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.decay", "must be > 0"));
    }

    #[test]
    fn dynamic_decay_missing_is_error() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(500), decay: None, minimum: Some(10), ..Default::default() });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.decay", "required"));
    }

    #[test]
    fn dynamic_missing_minimum_is_warning() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(500), decay: Some(50), minimum: None, ..Default::default() });
        let report = validate_challenges(&[c]);
        assert!(has_issue(&report, "extra.minimum", "not set"));
        assert!(warnings(&report).iter().any(|i| i.field.as_deref() == Some("extra.minimum")));
    }

    #[test]
    fn dynamic_all_valid_no_errors() {
        let mut c = make_challenge("test");
        c.challenge_type = ChallengeType::Dynamic;
        c.extra = Some(Extra { initial: Some(500), decay: Some(50), minimum: Some(100), ..Default::default() });
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
