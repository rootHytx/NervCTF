use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use nervctf::{
    challenge_manager::sync::needs_update,
    ctfd_api::{
        models::{Challenge, FlagContent, HintContent, State, Tag},
        CtfdClient,
    },
    directory_scanner::DirectoryScanner,
    fix::run_fix,
    load_config,
    setup::run_setup,
    validator::validate_challenges,
};
use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "nervctf")]
#[command(version = "0.1.0")]
#[command(about = "Minimalistic CTFd Challenge Management CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Base directory path (defaults to current directory)
    #[arg(short, long, default_value = ".")]
    base_dir: PathBuf,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Run remote monitor via Docker Compose
    #[arg(short, long)]
    remote: bool,

    /// Remote monitor URL (overrides MONITOR_URL env var and .nervctf.yml)
    #[arg(long)]
    monitor_url: Option<String>,

    /// Remote monitor token (overrides MONITOR_TOKEN env var and .nervctf.yml)
    #[arg(long)]
    monitor_token: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Deploy and sync challenges: creates new ones and updates changed ones
    Deploy {
        /// Show diff without applying changes
        #[arg(short, long)]
        dry_run: bool,
    },

    /// List all challenges found locally
    List {
        /// Show detailed information
        #[arg(short, long)]
        detailed: bool,
    },

    /// Scan directory for challenges and print statistics
    Scan {
        /// Show per-challenge detail
        #[arg(short, long)]
        detailed: bool,
    },

    /// Set up a remote CTFd environment (Docker, plugins, SSH access)
    Setup,

    /// Scan and fix common issues in challenge.yml files
    Fix {
        /// Preview changes without modifying any files
        #[arg(short, long)]
        dry_run: bool,
    },

    /// Validate challenge YAML files and report errors/warnings
    Validate,
}

// ── Queue types for deferred phases ──────────────────────────────────────────

struct FileUploadJob {
    challenge_id: u32,
    source_path: String,
    files: Vec<String>,
}

struct ReqJob {
    challenge_id: u32,
    prereq_names: Vec<String>,
}

struct NextJob {
    challenge_id: u32,
    next_name: String,
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Commands that don't need CTFd credentials
    if let Commands::Setup = cli.command {
        return run_setup();
    }
    if let Commands::Fix { dry_run } = cli.command {
        return run_fix(&cli.base_dir, dry_run);
    }
    if let Commands::Validate = cli.command {
        return validate_command(&cli.base_dir);
    }

    // Load config from .nervctf.yml (walk up from base_dir)
    let file_config = load_config(&cli.base_dir);

    // Resolve CTFD_URL: env var > config file
    let ctfd_url = env::var("CTFD_URL")
        .ok()
        .or_else(|| file_config.ctfd_url.clone())
        .ok_or_else(|| anyhow!("CTFD_URL is required (set via env var or .nervctf.yml)"))?;

    // Resolve CTFD_API_KEY: env var > config file
    let api_key = env::var("CTFD_API_KEY")
        .ok()
        .or_else(|| file_config.ctfd_api_key.clone())
        .ok_or_else(|| anyhow!("CTFD_API_KEY is required (set via env var or .nervctf.yml)"))?;

    // Resolve monitor config: CLI flag > env var > config file
    let monitor_url = cli
        .monitor_url
        .or_else(|| env::var("MONITOR_URL").ok())
        .or_else(|| file_config.monitor_url.clone());

    let monitor_token = cli
        .monitor_token
        .or_else(|| env::var("MONITOR_TOKEN").ok())
        .or_else(|| file_config.monitor_token.clone());

    if cli.verbose {
        if let Some(ref url) = monitor_url {
            println!("✅ Using remote monitor at {}", url);
        } else {
            println!("✅ Connected to CTFd at {}", ctfd_url);
        }
        println!("📁 Base directory: {}", cli.base_dir.display());
    }

    if cli.remote {
        let status = std::process::Command::new("docker")
            .args(["compose", "-f", "./remote-monitor/docker-compose.yml", "up"])
            .status()
            .expect("Failed to run docker compose");
        std::process::exit(status.code().unwrap_or(1));
    }

    // When monitor_url+token are both set, proxy through the monitor
    let client = if let (Some(ref url), Some(ref token)) = (&monitor_url, &monitor_token) {
        CtfdClient::new(url, token)?
    } else {
        CtfdClient::new(&ctfd_url, &api_key)?
    };

    let scanner = DirectoryScanner::new();

    match cli.command {
        Commands::Deploy { dry_run } => {
            deploy_challenges(&client, &scanner, &cli.base_dir, dry_run).await?;
        }
        Commands::List { detailed } => {
            list_challenges(&scanner, &cli.base_dir, detailed).await?;
        }
        Commands::Scan { detailed } => {
            scan_challenges(&scanner, &cli.base_dir, detailed).await?;
        }
        Commands::Setup | Commands::Fix { .. } | Commands::Validate => {
            unreachable!("handled before credential resolution")
        }
    }

    Ok(())
}

// ── validate ──────────────────────────────────────────────────────────────────

fn validate_command(base_dir: &PathBuf) -> Result<()> {
    let scanner = DirectoryScanner::new();
    let challenges = scanner.scan_directory(base_dir)?;

    if challenges.is_empty() {
        println!("ℹ️  No challenge files found in {}", base_dir.display());
        return Ok(());
    }

    println!("🔍 Validating {} challenge(s)...\n", challenges.len());
    let report = validate_challenges(&challenges);
    report.print();

    if report.has_errors() {
        std::process::exit(1);
    }
    Ok(())
}

// ── deploy ────────────────────────────────────────────────────────────────────

async fn deploy_challenges(
    client: &CtfdClient,
    scanner: &DirectoryScanner,
    base_dir: &PathBuf,
    dry_run: bool,
) -> Result<()> {
    use std::path::Path;

    // ── Gather local and remote challenges ────────────────────────────────────
    let local_challenges = scanner.scan_directory(base_dir)?;
    if local_challenges.is_empty() {
        println!("ℹ️  No challenge files found in {}", base_dir.display());
        return Ok(());
    }
    println!("📊 Local:  {} challenge(s)", local_challenges.len());

    // ── Pre-deploy validation ─────────────────────────────────────────────────
    println!("\n🔍 Validating challenges...");
    let report = validate_challenges(&local_challenges);
    report.print();
    if report.has_errors() {
        println!("\n❌ Fix the errors above before deploying. Run `nervctf validate` for details.");
        return Ok(());
    }
    if !report.is_clean() {
        println!();
    }

    let remote_challenges = client.get_challenges().await?.unwrap_or_default();
    println!("📊 Remote: {} challenge(s)", remote_challenges.len());

    let remote_map: HashMap<String, &Challenge> = remote_challenges
        .iter()
        .map(|c| (c.name.clone(), c))
        .collect();

    // ── Compute diff ──────────────────────────────────────────────────────────
    let mut to_create: Vec<&Challenge> = Vec::new();
    let mut to_update: Vec<(&Challenge, u32)> = Vec::new();
    let mut up_to_date_names: Vec<&str> = Vec::new();

    for local in &local_challenges {
        if let Some(remote) = remote_map.get(&local.name) {
            if needs_update(remote, local) {
                let remote_id = remote
                    .id
                    .ok_or_else(|| anyhow!("Remote challenge '{}' has no ID", local.name))?;
                to_update.push((local, remote_id));
            } else {
                up_to_date_names.push(&local.name);
            }
        } else {
            to_create.push(local);
        }
    }

    // ── Show diff ─────────────────────────────────────────────────────────────
    println!("\n📋 Diff:");
    println!("{}", "=".repeat(50));
    if !to_create.is_empty() {
        println!("➕ CREATE ({}):", to_create.len());
        for c in &to_create {
            println!("    - {}", c.name);
        }
    }
    if !to_update.is_empty() {
        println!("🔄 UPDATE ({}):", to_update.len());
        for (c, _) in &to_update {
            println!("    - {}", c.name);
        }
    }
    if !up_to_date_names.is_empty() {
        println!("✅ UP-TO-DATE ({}):", up_to_date_names.len());
        for name in &up_to_date_names {
            println!("    - {}", name);
        }
    }
    println!("{}", "=".repeat(50));

    if dry_run {
        println!("ℹ️  Dry run — no changes applied.");
        return Ok(());
    }

    if to_create.is_empty() && to_update.is_empty() {
        println!("✅ Everything is up to date.");
        return Ok(());
    }

    print!("Proceed? (y/N): ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" {
        println!("❌ Aborted.");
        return Ok(());
    }

    // ── Phase 1: core + flags + tags + topics + hints ─────────────────────────
    println!("\n🚀 Phase 1: cores, flags, tags, hints...");
    let mut file_jobs: Vec<FileUploadJob> = Vec::new();
    let mut req_jobs: Vec<ReqJob> = Vec::new();
    let mut next_jobs: Vec<NextJob> = Vec::new();
    let mut created = 0usize;
    let mut updated = 0usize;

    for local in &to_create {
        print!("  ➕ {}: ", local.name);
        std::io::stdout().flush()?;
        match create_challenge_phase1(client, local).await {
            Ok((id, has_files, has_reqs, has_next)) => {
                println!("✅ (ID {})", id);
                created += 1;
                if has_files {
                    file_jobs.push(FileUploadJob {
                        challenge_id: id,
                        source_path: local.source_path.clone(),
                        files: local.files.as_ref().cloned().unwrap_or_default(),
                    });
                }
                if has_reqs {
                    req_jobs.push(ReqJob {
                        challenge_id: id,
                        prereq_names: local.requirements.as_ref().unwrap().prerequisite_names(),
                    });
                }
                if let Some(next_name) = has_next {
                    next_jobs.push(NextJob {
                        challenge_id: id,
                        next_name,
                    });
                }
            }
            Err(e) => eprintln!("❌ {}", e),
        }
    }

    for (local, remote_id) in &to_update {
        print!("  🔄 {}: ", local.name);
        std::io::stdout().flush()?;
        match update_challenge_phase1(client, *remote_id, local).await {
            Ok((has_files, has_reqs, has_next)) => {
                println!("✅ (ID {})", remote_id);
                updated += 1;
                if has_files {
                    file_jobs.push(FileUploadJob {
                        challenge_id: *remote_id,
                        source_path: local.source_path.clone(),
                        files: local.files.as_ref().cloned().unwrap_or_default(),
                    });
                }
                if has_reqs {
                    req_jobs.push(ReqJob {
                        challenge_id: *remote_id,
                        prereq_names: local.requirements.as_ref().unwrap().prerequisite_names(),
                    });
                }
                if let Some(next_name) = has_next {
                    next_jobs.push(NextJob {
                        challenge_id: *remote_id,
                        next_name,
                    });
                }
            }
            Err(e) => eprintln!("❌ {}", e),
        }
    }

    // ── Phase 2: file uploads ─────────────────────────────────────────────────
    // All files for a challenge are sent in a SINGLE multipart request,
    // matching ctfcli's _create_all_files() approach (multiple "file" parts,
    // plus "challenge_id" and "type" as form text fields).
    if !file_jobs.is_empty() {
        println!("\n📁 Phase 2: uploading files...");
        for job in &file_jobs {
            // Collect all existing files for this challenge
            let mut file_parts: Vec<(String, reqwest::multipart::Part)> = Vec::new();
            let mut missing: Vec<&str> = Vec::new();

            for file in &job.files {
                let file_path = Path::new(&job.source_path).join(file);
                if file_path.exists() {
                    let file_bytes = tokio::fs::read(&file_path).await?;
                    let filename = file_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file.as_str())
                        .to_string();
                    let part = reqwest::multipart::Part::bytes(file_bytes).file_name(filename);
                    file_parts.push((file.clone(), part));
                } else {
                    missing.push(file.as_str());
                }
            }

            for f in &missing {
                eprintln!("  ⚠️  Not found: {}/{}", job.source_path, f);
            }

            if file_parts.is_empty() {
                continue;
            }

            // Build one form with all files (same "file" field name for each)
            let mut form = reqwest::multipart::Form::new()
                .text("challenge_id", job.challenge_id.to_string())
                .text("type", "challenge");
            let names: Vec<String> = file_parts.iter().map(|(n, _)| n.clone()).collect();
            for (_, part) in file_parts {
                form = form.part("file", part);
            }

            match client.upload_file("/files", form).await {
                Ok(_) => println!(
                    "  ✅ Uploaded {} file(s) → challenge {}: {}",
                    names.len(),
                    job.challenge_id,
                    names.join(", ")
                ),
                Err(e) => eprintln!("  ❌ Upload for challenge {}: {}", job.challenge_id, e),
            }
        }
    }

    // ── Phase 3: requirements ─────────────────────────────────────────────────
    if !req_jobs.is_empty() {
        println!("\n🔗 Phase 3: patching requirements...");
        let all_remote = client.get_challenges().await?.unwrap_or_default();
        let name_to_id: HashMap<String, u32> = all_remote
            .iter()
            .filter_map(|c| c.id.map(|id| (c.name.clone(), id)))
            .collect();

        for job in &req_jobs {
            let mut prereq_ids: Vec<u32> = Vec::new();
            for req in &job.prereq_names {
                if req.trim().is_empty() {
                    continue;
                }
                if let Ok(id) = req.parse::<u32>() {
                    prereq_ids.push(id);
                } else if let Some(&id) = name_to_id.get(req.as_str()) {
                    prereq_ids.push(id);
                } else {
                    eprintln!("  ⚠️  Prerequisite '{}' not found on remote", req);
                }
            }
            if !prereq_ids.is_empty() {
                let req_data =
                    serde_json::json!({ "requirements": { "prerequisites": prereq_ids } });
                match client.update_challenge(job.challenge_id, &req_data).await {
                    Ok(_) => println!("  ✅ Requirements set for challenge {}", job.challenge_id),
                    Err(e) => eprintln!("  ❌ Requirements for {}: {}", job.challenge_id, e),
                }
            }
        }
    }

    // ── Phase 4: next pointers ────────────────────────────────────────────────
    if !next_jobs.is_empty() {
        println!("\n➡️  Phase 4: patching next pointers...");
        let all_remote = client.get_challenges().await?.unwrap_or_default();
        let name_to_id: HashMap<String, u32> = all_remote
            .iter()
            .filter_map(|c| c.id.map(|id| (c.name.clone(), id)))
            .collect();

        for job in &next_jobs {
            if let Some(&next_id) = name_to_id.get(job.next_name.as_str()) {
                let next_data = serde_json::json!({ "next_id": next_id });
                match client.update_challenge(job.challenge_id, &next_data).await {
                    Ok(_) => println!(
                        "  ✅ next → '{}' (ID {}) for challenge {}",
                        job.next_name, next_id, job.challenge_id
                    ),
                    Err(e) => eprintln!("  ❌ next for {}: {}", job.challenge_id, e),
                }
            } else {
                eprintln!(
                    "  ⚠️  Next challenge '{}' not found on remote",
                    job.next_name
                );
            }
        }
    }

    println!("\n✅ Done!");
    println!("   Created:    {}", created);
    println!("   Updated:    {}", updated);
    println!("   Up-to-date: {}", up_to_date_names.len());

    Ok(())
}

/// Phase 1 for a new challenge: POST core, flags, tags, topics, hints.
/// Returns `(id, has_files, has_requirements, next_challenge_name)`.
async fn create_challenge_phase1(
    client: &CtfdClient,
    challenge: &Challenge,
) -> Result<(u32, bool, bool, Option<String>)> {
    use reqwest::Method;
    use serde_json::json;

    let state_str = match challenge.state.as_ref().unwrap_or(&State::Visible) {
        State::Visible => "visible",
        State::Hidden => "hidden",
    };

    let mut payload = json!({
        "name":        challenge.name,
        "category":    challenge.category,
        "description": challenge.description.as_deref().unwrap_or(""),
        "value":       challenge.value,
        "type":        challenge.challenge_type,
        "state":       state_str,
    });
    if let Some(ref ci) = challenge.connection_info {
        payload["connection_info"] = json!(ci);
    }
    if let Some(attempts) = challenge.attempts {
        payload["attempts"] = json!(attempts);
    }
    if let Some(ref extra) = challenge.extra {
        payload["extra"] = serde_json::to_value(extra).unwrap_or_default();
    }

    let created: Challenge = client
        .execute::<Challenge, _>(Method::POST, "/challenges", Some(&payload))
        .await?
        .ok_or_else(|| anyhow!("No response body from challenge creation"))?;
    let id = created
        .id
        .ok_or_else(|| anyhow!("Created challenge returned no ID"))?;

    post_flags(client, id, &challenge.flags).await?;
    post_tags(client, id, &challenge.tags).await?;
    post_topics(client, id, &challenge.topics).await?;
    post_hints(client, id, &challenge.hints).await?;

    let has_files = challenge
        .files
        .as_ref()
        .map(|f| !f.is_empty())
        .unwrap_or(false);
    let has_reqs = challenge.requirements.is_some();
    Ok((id, has_files, has_reqs, challenge.next.clone()))
}

/// Phase 1 for an existing challenge: PATCH core, replace flags/tags/hints, delete old files.
/// Returns `(has_files, has_requirements, next_challenge_name)`.
async fn update_challenge_phase1(
    client: &CtfdClient,
    challenge_id: u32,
    challenge: &Challenge,
) -> Result<(bool, bool, Option<String>)> {
    use serde_json::{json, Value};

    let state_str = match challenge.state.as_ref().unwrap_or(&State::Visible) {
        State::Visible => "visible",
        State::Hidden => "hidden",
    };

    let mut payload = json!({
        "name":        challenge.name,
        "category":    challenge.category,
        "description": challenge.description.as_deref().unwrap_or(""),
        "value":       challenge.value,
        "type":        challenge.challenge_type,
        "state":       state_str,
    });
    if let Some(ref ci) = challenge.connection_info {
        payload["connection_info"] = json!(ci);
    }
    if let Some(attempts) = challenge.attempts {
        payload["attempts"] = json!(attempts);
    }
    if let Some(ref extra) = challenge.extra {
        payload["extra"] = serde_json::to_value(extra).unwrap_or_default();
    }
    client.update_challenge(challenge_id, &payload).await?;

    // Replace flags (best-effort: skip if CTFd sub-endpoints are unavailable,
    // e.g. when Challenge Visibility is set to Private in CTFd Admin → Config)
    match client.get_challenge_flags_endpoint(challenge_id).await {
        Ok(Some(existing)) => {
            for flag in existing.as_array().unwrap_or(&vec![]) {
                if let Some(flag_id) = flag.get("id").and_then(Value::as_u64) {
                    client.delete_flag(flag_id as u32).await?;
                }
            }
            post_flags(client, challenge_id, &challenge.flags).await?;
        }
        Ok(None) => {
            post_flags(client, challenge_id, &challenge.flags).await?;
        }
        Err(e) => {
            return Err(visibility_or_err(e, "flags", challenge_id));
        }
    }

    // Replace tags
    match client.get_challenge_tags_endpoint(challenge_id).await {
        Ok(Some(existing)) => {
            for tag in existing.as_array().unwrap_or(&vec![]) {
                if let Some(tag_id) = tag.get("id").and_then(Value::as_u64) {
                    client.delete_tag(tag_id as u32).await?;
                }
            }
            post_tags(client, challenge_id, &challenge.tags).await?;
        }
        Ok(None) => {
            post_tags(client, challenge_id, &challenge.tags).await?;
        }
        Err(e) => {
            return Err(visibility_or_err(e, "tags", challenge_id));
        }
    }
    post_topics(client, challenge_id, &challenge.topics).await?;

    // Replace hints
    match client.get_challenge_hints_endpoint(challenge_id).await {
        Ok(Some(existing)) => {
            for hint in existing.as_array().unwrap_or(&vec![]) {
                if let Some(hint_id) = hint.get("id").and_then(Value::as_u64) {
                    client.delete_hint(hint_id as u32).await?;
                }
            }
            post_hints(client, challenge_id, &challenge.hints).await?;
        }
        Ok(None) => {
            post_hints(client, challenge_id, &challenge.hints).await?;
        }
        Err(e) => {
            return Err(visibility_or_err(e, "hints", challenge_id));
        }
    }

    // Delete existing files; new ones will be uploaded in Phase 2
    match client.get_challenge_files_endpoint(challenge_id).await {
        Ok(Some(existing)) => {
            for file in existing.as_array().unwrap_or(&vec![]) {
                if let Some(file_id) = file.get("id").and_then(Value::as_u64) {
                    client.delete_file(file_id as u32).await?;
                }
            }
        }
        Ok(None) => {}
        Err(e) => {
            return Err(visibility_or_err(e, "files", challenge_id));
        }
    }

    let has_files = challenge
        .files
        .as_ref()
        .map(|f| !f.is_empty())
        .unwrap_or(false);
    let has_reqs = challenge.requirements.is_some();
    Ok((has_files, has_reqs, challenge.next.clone()))
}

// ── Small helpers ─────────────────────────────────────────────────────────────

/// Converts a sub-endpoint error to a helpful message when it is a CTFd login
/// redirect (which happens when Challenge Visibility is set to Private).
fn visibility_or_err(e: anyhow::Error, resource: &str, id: u32) -> anyhow::Error {
    let msg = e.to_string();
    if msg.contains("/login") || msg.contains("Redirecting") {
        anyhow::anyhow!(
            "challenge {} {}: CTFd returned a login redirect — \
             set Challenge Visibility to Public in CTFd Admin → Config → Visibility",
            id,
            resource
        )
    } else {
        e
    }
}

async fn post_flags(
    client: &CtfdClient,
    challenge_id: u32,
    flags: &Option<Vec<FlagContent>>,
) -> Result<()> {
    use reqwest::Method;
    use serde_json::json;
    if let Some(flags) = flags {
        for flag in flags {
            let data = match flag {
                FlagContent::Simple(content) => json!({
                    "challenge_id": challenge_id,
                    "content":      content,
                    "type":         "static",
                    "data":         "",
                }),
                FlagContent::Detailed {
                    type_,
                    content,
                    data,
                    ..
                } => {
                    let data_val = data
                        .as_ref()
                        .map(|d| serde_json::to_value(d).unwrap_or_default())
                        .unwrap_or(serde_json::Value::String(String::new()));
                    json!({
                        "challenge_id": challenge_id,
                        "content":      content,
                        "type":         serde_json::to_value(type_).unwrap_or_default(),
                        "data":         data_val,
                    })
                }
            };
            client
                .execute::<serde_json::Value, _>(Method::POST, "/flags", Some(&data))
                .await?;
        }
    }
    Ok(())
}

async fn post_tags(client: &CtfdClient, challenge_id: u32, tags: &Option<Vec<Tag>>) -> Result<()> {
    use reqwest::Method;
    use serde_json::json;
    if let Some(tags) = tags {
        for tag in tags {
            let value = match tag {
                Tag::Simple(s) => s.as_str(),
                Tag::Detailed { value, .. } => value.as_str(),
            };
            client
                .execute::<serde_json::Value, _>(
                    Method::POST,
                    "/tags",
                    Some(&json!({ "challenge_id": challenge_id, "value": value })),
                )
                .await?;
        }
    }
    Ok(())
}

async fn post_topics(
    client: &CtfdClient,
    challenge_id: u32,
    topics: &Option<Vec<String>>,
) -> Result<()> {
    use reqwest::Method;
    use serde_json::json;
    if let Some(topics) = topics {
        for topic in topics {
            client
                .execute::<serde_json::Value, _>(
                    Method::POST,
                    "/topics",
                    Some(&json!({
                        "challenge_id": challenge_id,
                        "value":        topic,
                        "type":         "challenge",
                    })),
                )
                .await?;
        }
    }
    Ok(())
}

async fn post_hints(
    client: &CtfdClient,
    challenge_id: u32,
    hints: &Option<Vec<HintContent>>,
) -> Result<()> {
    use reqwest::Method;
    use serde_json::json;
    if let Some(hints) = hints {
        for hint in hints {
            let (content, cost) = match hint {
                HintContent::Simple(s) => (s.as_str(), 0u32),
                HintContent::Detailed { content, cost, .. } => {
                    (content.as_str(), cost.unwrap_or(0))
                }
            };
            client
                .execute::<serde_json::Value, _>(
                    Method::POST,
                    "/hints",
                    Some(&json!({
                        "challenge_id": challenge_id,
                        "content":      content,
                        "cost":         cost,
                    })),
                )
                .await?;
        }
    }
    Ok(())
}

// ── list / scan ───────────────────────────────────────────────────────────────

async fn list_challenges(
    scanner: &DirectoryScanner,
    base_dir: &PathBuf,
    detailed: bool,
) -> Result<()> {
    let challenges = scanner.scan_directory(base_dir)?;

    if challenges.is_empty() {
        println!("ℹ️  No challenge files found");
        return Ok(());
    }

    println!("📋 Found {} challenges:", challenges.len());
    for challenge in challenges {
        if detailed {
            match challenge.to_yaml_string() {
                Ok(yaml) => println!("{}", yaml),
                Err(e) => println!("Failed to serialize: {}", e),
            }
        } else {
            println!(
                "  - {} ({}) — {} pts",
                challenge.name, challenge.category, challenge.value
            );
        }
    }
    Ok(())
}

async fn scan_challenges(
    scanner: &DirectoryScanner,
    base_dir: &PathBuf,
    detailed: bool,
) -> Result<()> {
    let challenges = scanner.scan_directory(base_dir)?;

    if challenges.is_empty() {
        println!("ℹ️  No challenge files found");
        return Ok(());
    }

    if detailed {
        println!("\n📋 {} challenges:", challenges.len());
        for c in &challenges {
            println!("  - {} ({}) — {} pts", c.name, c.category, c.value);
            if let Some(flags) = &c.flags {
                println!("    Flags: {}", flags.len());
            }
            if let Some(hints) = &c.hints {
                println!("    Hints: {}", hints.len());
            }
        }
    } else {
        println!("📋 Found {} challenges", challenges.len());
    }

    let stats = scanner.get_stats(&challenges);
    stats.print();
    Ok(())
}
