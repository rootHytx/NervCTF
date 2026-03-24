use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use serde_json::json;
use nervctf::{
    challenge_manager::sync::needs_update,
    ctfd_api::{
        models::{Challenge, ChallengeType, FlagContent, HintContent, State, Tag},
        CtfdClient,
    },
    directory_scanner::DirectoryScanner,
    fix::run_fix,
    load_config,
    setup::{run_setup, run_upgrade},
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
    Setup {
        /// Upgrade an existing deployment: push new plugin + binary, rebuild image, restart containers
        #[arg(long)]
        upgrade: bool,
    },

    /// Scan and fix common issues in challenge.yml files
    Fix {
        /// Preview changes without modifying any files
        #[arg(short, long)]
        dry_run: bool,
    },

    /// Validate challenge YAML files and report errors/warnings
    Validate {
        /// Show full field-by-field dictionary for every challenge
        #[arg(long)]
        debug: bool,
    },
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

    // Setup/upgrade have no config and no base_dir — handle them first.
    if let Commands::Setup { upgrade } = cli.command {
        if upgrade {
            return run_upgrade();
        }
        return run_setup();
    }

    // Load config from .nervctf.yml — always search from CWD upwards so the
    // file is found regardless of what --base-dir is set to.
    let cwd = env::current_dir().unwrap_or_else(|_| cli.base_dir.clone());
    let (file_config, config_path) = load_config(&cwd);

    // Resolve effective base_dir: CLI flag > .nervctf.yml > "."
    // We treat the clap default (".") as "not explicitly set", so the config
    // file can override it.
    let effective_base_dir: PathBuf = if cli.base_dir != PathBuf::from(".") {
        cli.base_dir.clone() // explicitly passed by the user
    } else {
        file_config
            .base_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or(cli.base_dir.clone())
    };

    // Commands that only need base_dir (no CTFd credentials required)
    if let Commands::Fix { dry_run } = cli.command {
        return run_fix(&effective_base_dir, dry_run);
    }
    if let Commands::Validate { debug } = cli.command {
        return validate_command(&effective_base_dir, debug);
    }

    // Resolve monitor config: CLI flag > env var > config file (required)
    let monitor_url = cli
        .monitor_url
        .or_else(|| env::var("MONITOR_URL").ok())
        .or_else(|| file_config.monitor_url.clone())
        .ok_or_else(|| anyhow!("MONITOR_URL is required (set via --monitor-url, MONITOR_URL env var, or .nervctf.yml)"))?;

    let monitor_token = cli
        .monitor_token
        .or_else(|| env::var("MONITOR_TOKEN").ok())
        .or_else(|| file_config.monitor_token.clone())
        .ok_or_else(|| anyhow!("MONITOR_TOKEN is required (set via --monitor-token, MONITOR_TOKEN env var, or .nervctf.yml)"))?;

    if cli.verbose {
        match &config_path {
            Some(p) => println!("config: {}", p.display()),
            None => println!("config: none found (using env vars / CLI flags only)"),
        }
        println!("monitor: {}", monitor_url);
        println!("base-dir: {}", effective_base_dir.display());
    }

    if cli.remote {
        let status = std::process::Command::new("docker")
            .args(["compose", "-f", "./remote-monitor/docker-compose.yml", "up"])
            .status()
            .expect("Failed to run docker compose");
        std::process::exit(status.code().unwrap_or(1));
    }

    let client = CtfdClient::new(&monitor_url, &monitor_token)?;

    let scanner = DirectoryScanner::new();

    match cli.command {
        Commands::Deploy { dry_run } => {
            deploy_challenges(&client, &monitor_url, &scanner, &effective_base_dir, dry_run).await?;
        }
        Commands::List { detailed } => {
            list_challenges(&scanner, &effective_base_dir, detailed).await?;
        }
        Commands::Scan { detailed } => {
            scan_challenges(&scanner, &effective_base_dir, detailed).await?;
        }
        Commands::Setup { .. } | Commands::Fix { .. } | Commands::Validate { .. } => {
            unreachable!("handled before credential resolution")
        }
    }

    Ok(())
}

// ── validate ──────────────────────────────────────────────────────────────────

fn validate_command(base_dir: &PathBuf, debug: bool) -> Result<()> {
    use nervctf::ScanFailure;

    let scanner = DirectoryScanner::new();
    let (challenges, failures): (_, Vec<ScanFailure>) =
        scanner.scan_directory_full(base_dir, debug)?;

    if challenges.is_empty() && failures.is_empty() {
        println!("note: no challenge files found in {}", base_dir.display());
        return Ok(());
    }

    let total = challenges.len() + failures.len();
    if failures.is_empty() {
        println!("validating {} challenge(s)...\n", total);
    } else {
        println!(
            "validating {} challenge(s) ({} failed to parse)...\n",
            total,
            failures.len()
        );
    }

    let report = validate_challenges(&challenges);
    report.print(&challenges, &failures, debug);

    if report.has_errors() || !failures.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

// ── deploy ────────────────────────────────────────────────────────────────────

async fn deploy_challenges(
    client: &CtfdClient,
    monitor_url: &str,
    scanner: &DirectoryScanner,
    base_dir: &PathBuf,
    dry_run: bool,
) -> Result<()> {
    use std::path::Path;

    let local_challenges = scanner.scan_directory(base_dir)?;
    if local_challenges.is_empty() {
        println!("note: no challenge files found in {}", base_dir.display());
        return Ok(());
    }
    println!("local:  {} challenge(s)", local_challenges.len());

    println!("\nvalidating challenges...");
    let report = validate_challenges(&local_challenges);
    let no_failures: Vec<nervctf::ScanFailure> = vec![];
    report.print(&local_challenges, &no_failures, false);
    if report.has_errors() {
        println!("\n[x] fix the errors above before deploying. run `nervctf validate` for details.");
        return Ok(());
    }
    if !report.is_clean() {
        println!();
    }

    let remote_challenges = client.get_challenges().await?.unwrap_or_default();
    println!("remote: {} challenge(s)", remote_challenges.len());

    let remote_map: HashMap<String, &Challenge> = remote_challenges
        .iter()
        .map(|c| (c.name.clone(), c))
        .collect();

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

    println!("\ndiff:");
    println!("{}", "=".repeat(50));
    if !to_create.is_empty() {
        println!("[+] CREATE ({}):", to_create.len());
        for c in &to_create { println!("    - {}", c.name); }
    }
    if !to_update.is_empty() {
        println!("[~] UPDATE ({}):", to_update.len());
        for (c, _) in &to_update { println!("    - {}", c.name); }
    }
    if !up_to_date_names.is_empty() {
        println!("[=] UP-TO-DATE ({}):", up_to_date_names.len());
        for name in &up_to_date_names { println!("    - {}", name); }
    }
    println!("{}", "=".repeat(50));

    if dry_run {
        println!("note: dry run -- no changes applied.");
        return Ok(());
    }

    if to_create.is_empty() && to_update.is_empty() {
        println!("everything is up to date.");
        return Ok(());
    }

    print!("Proceed? (y/N): ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" {
        println!("aborted.");
        return Ok(());
    }

    println!("\n-- phase 1: cores, flags, tags, hints");
    let mut file_jobs: Vec<FileUploadJob> = Vec::new();
    let mut req_jobs: Vec<ReqJob> = Vec::new();
    let mut next_jobs: Vec<NextJob> = Vec::new();
    let mut created = 0usize;
    let mut updated = 0usize;

    for local in &to_create {
        print!("  [+] {}: ", local.name);
        std::io::stdout().flush()?;
        match create_challenge_phase1(client, local).await {
            Ok((id, has_files, has_reqs, has_next)) => {
                println!("ok (ID {})", id);
                created += 1;
                if local.challenge_type == ChallengeType::Instance {
                    if let Err(e) = deploy_instance(client, local, id).await {
                        eprintln!("  [!] instance deploy error for '{}': {}", local.name, e);
                    }
                }
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
                    next_jobs.push(NextJob { challenge_id: id, next_name });
                }
            }
            Err(e) => eprintln!("[x] {}", e),
        }
    }

    for (local, remote_id) in &to_update {
        print!("  [~] {}: ", local.name);
        std::io::stdout().flush()?;
        match update_challenge_phase1(client, *remote_id, local).await {
            Ok((has_files, has_reqs, has_next)) => {
                println!("ok (ID {})", remote_id);
                updated += 1;
                if local.challenge_type == ChallengeType::Instance {
                    if let Err(e) = deploy_instance(client, local, *remote_id).await {
                        eprintln!("  [!] instance deploy error for '{}': {}", local.name, e);
                    }
                }
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
                    next_jobs.push(NextJob { challenge_id: *remote_id, next_name });
                }
            }
            Err(e) => eprintln!("[x] {}", e),
        }
    }

    if !file_jobs.is_empty() {
        println!("\n-- phase 2: uploading files");
        for job in &file_jobs {
            let mut file_parts: Vec<(String, reqwest::multipart::Part)> = Vec::new();
            let mut missing: Vec<&str> = Vec::new();
            for file in &job.files {
                let file_path = Path::new(&job.source_path).join(file);
                if file_path.exists() {
                    let file_bytes = tokio::fs::read(&file_path).await?;
                    let filename = file_path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file.as_str())
                        .to_string();
                    let part = reqwest::multipart::Part::bytes(file_bytes).file_name(filename);
                    file_parts.push((file.clone(), part));
                } else {
                    missing.push(file.as_str());
                }
            }
            for f in &missing { eprintln!("  [!] not found: {}/{}", job.source_path, f); }
            if file_parts.is_empty() { continue; }
            let mut form = reqwest::multipart::Form::new()
                .text("challenge_id", job.challenge_id.to_string())
                .text("type", "challenge");
            let names: Vec<String> = file_parts.iter().map(|(n, _)| n.clone()).collect();
            for (_, part) in file_parts { form = form.part("file", part); }
            match client.upload_file("/files", form).await {
                Ok(_) => println!("  [+] uploaded {} file(s) -> challenge {}: {}", names.len(), job.challenge_id, names.join(", ")),
                Err(e) => eprintln!("  [x] upload for challenge {}: {}", job.challenge_id, e),
            }
        }
    }

    if !req_jobs.is_empty() {
        println!("\n-- phase 3: patching requirements");
        let all_remote = client.get_challenges().await?.unwrap_or_default();
        let name_to_id: HashMap<String, u32> = all_remote.iter()
            .filter_map(|c| c.id.map(|id| (c.name.clone(), id)))
            .collect();
        for job in &req_jobs {
            let mut prereq_ids: Vec<u32> = Vec::new();
            for req in &job.prereq_names {
                if req.trim().is_empty() { continue; }
                if let Ok(id) = req.parse::<u32>() {
                    prereq_ids.push(id);
                } else if let Some(&id) = name_to_id.get(req.as_str()) {
                    prereq_ids.push(id);
                } else {
                    eprintln!("  [!] prerequisite '{}' not found on remote", req);
                }
            }
            if !prereq_ids.is_empty() {
                let req_data = serde_json::json!({ "requirements": { "prerequisites": prereq_ids } });
                match client.update_challenge(job.challenge_id, &req_data).await {
                    Ok(_) => println!("  [ok] requirements set for challenge {}", job.challenge_id),
                    Err(e) => eprintln!("  [x] requirements for {}: {}", job.challenge_id, e),
                }
            }
        }
    }

    if !next_jobs.is_empty() {
        println!("\n-- phase 4: patching next pointers");
        let all_remote = client.get_challenges().await?.unwrap_or_default();
        let name_to_id: HashMap<String, u32> = all_remote.iter()
            .filter_map(|c| c.id.map(|id| (c.name.clone(), id)))
            .collect();
        for job in &next_jobs {
            if let Some(&next_id) = name_to_id.get(job.next_name.as_str()) {
                let next_data = serde_json::json!({ "next_id": next_id });
                match client.update_challenge(job.challenge_id, &next_data).await {
                    Ok(_) => println!("  [ok] next -> '{}' (ID {}) for challenge {}", job.next_name, next_id, job.challenge_id),
                    Err(e) => eprintln!("  [x] next for {}: {}", job.challenge_id, e),
                }
            } else {
                eprintln!("  [!] next challenge '{}' not found on remote", job.next_name);
            }
        }
    }

    println!("\ndone.");
    println!("   Created:    {}", created);
    println!("   Updated:    {}", updated);
    println!("   Up-to-date: {}", up_to_date_names.len());

    let _ = monitor_url; // used only for logging if needed
    Ok(())
}

/// Spread `InstanceConfig` fields as top-level keys in a JSON payload so the
/// nervctf_instance CTFd plugin can read them from `request.get_json()`.
fn merge_instance_fields(
    payload: &mut serde_json::Value,
    instance: &Option<nervctf::ctfd_api::models::InstanceConfig>,
) {
    let Some(inst) = instance else { return };
    if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(inst) {
        for (k, v) in map {
            if v != serde_json::Value::Null {
                payload[k] = v;
            }
        }
    }
}

/// Spread `Extra` fields as top-level keys in a JSON payload.
/// The `dynamic` plugin passes request data as `**kwargs` to the SQLAlchemy model
/// constructor — it doesn't accept a nested `extra` dict.
fn merge_extra_fields(payload: &mut serde_json::Value, extra: &nervctf::ctfd_api::models::Extra) {
    if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(extra) {
        for (k, v) in map {
            if v != serde_json::Value::Null {
                payload[k] = v;
            }
        }
    }
}

/// Determine the CTFd challenge type string and description for a challenge.
/// Instance challenges are deployed as `"instance"` (requires the nervctf_instance CTFd plugin).
fn resolve_challenge_type_and_description(
    challenge: &Challenge,
    _monitor_url: &Option<String>,
) -> (String, String) {
    let base_desc = challenge.description.as_deref().unwrap_or("").to_string();
    match challenge.challenge_type {
        ChallengeType::Instance => ("instance".to_string(), base_desc),
        ChallengeType::Dynamic => ("dynamic".to_string(), base_desc),
        ChallengeType::Standard => ("standard".to_string(), base_desc),
    }
}

/// Register an instance challenge with the remote monitor after CTFd creation/update.
async fn deploy_instance(
    monitor_client: &CtfdClient,
    challenge: &Challenge,
    ctfd_id: u32,
) -> Result<()> {
    use reqwest::Method;

    let inst = match &challenge.instance {
        Some(i) => i,
        None => return Ok(()), // no instance config, nothing to register
    };

    let backend_str = serde_json::to_value(&inst.backend)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "docker".to_string());

    let register_payload = json!({
        "challenge_name": challenge.name,
        "ctfd_id": ctfd_id,
        "backend": backend_str,
        "config_json": serde_json::to_string(inst)?,
    });

    match monitor_client
        .execute::<serde_json::Value, _>(
            Method::POST,
            "/instance/register",
            Some(&register_payload),
        )
        .await
    {
        Ok(_) => println!("   [ok] instance registered with monitor"),
        Err(e) => eprintln!("   [!] monitor registration failed for '{}': {}", challenge.name, e),
    }

    // If Compose backend with relative compose_file, upload challenge dir for pre-building
    if let nervctf::ctfd_api::models::InstanceBackend::Compose = inst.backend {
        if let Some(cf) = &inst.compose_file {
            let is_local = !cf.starts_with('/');
            if is_local {
                let context_dir = std::path::PathBuf::from(&challenge.source_path);
                match tokio::process::Command::new("tar")
                    .args(["-czf", "-", "."])
                    .current_dir(&context_dir)
                    .output()
                    .await
                {
                    Ok(out) if out.status.success() => {
                        let form = reqwest::multipart::Form::new()
                            .text("challenge_name", challenge.name.clone())
                            .part(
                                "context",
                                reqwest::multipart::Part::bytes(out.stdout)
                                    .file_name("context.tar.gz")
                                    .mime_str("application/gzip")
                                    .unwrap(),
                            );
                        match monitor_client.upload_file("/instance/build-compose", form).await {
                            Ok(_) => println!("   [ok] compose context uploaded and images built on monitor"),
                            Err(e) => eprintln!("   [!] monitor compose build failed for '{}': {}", challenge.name, e),
                        }
                    }
                    Ok(out) => eprintln!(
                        "   [!] tar failed for '{}': {}",
                        challenge.name,
                        String::from_utf8_lossy(&out.stderr)
                    ),
                    Err(e) => eprintln!(
                        "   [!] failed to create build context for '{}': {}",
                        challenge.name, e
                    ),
                }
            }
        }
    }

    // If Docker backend with local image path, pack and send the build context
    if let nervctf::ctfd_api::models::InstanceBackend::Docker = inst.backend {
        if let Some(img) = &inst.image {
            let is_local = img == "." || img.starts_with("./") || img.starts_with("../");
            if is_local {
                let context_dir = if img == "." {
                    std::path::PathBuf::from(&challenge.source_path)
                } else {
                    std::path::PathBuf::from(&challenge.source_path).join(img)
                };

                match tokio::process::Command::new("tar")
                    .args(["-czf", "-", "."])
                    .current_dir(&context_dir)
                    .output()
                    .await
                {
                    Ok(out) if out.status.success() => {
                        let form = reqwest::multipart::Form::new()
                            .text("challenge_name", challenge.name.clone())
                            .part(
                                "context",
                                reqwest::multipart::Part::bytes(out.stdout)
                                    .file_name("context.tar.gz")
                                    .mime_str("application/gzip")
                                    .unwrap(),
                            );
                        match monitor_client.upload_file("/instance/build", form).await {
                            Ok(_) => println!("   [ok] image build triggered on monitor"),
                            Err(e) => eprintln!("   [!] monitor build failed for '{}': {}", challenge.name, e),
                        }
                    }
                    Ok(out) => eprintln!(
                        "   [!] tar failed for '{}': {}",
                        challenge.name,
                        String::from_utf8_lossy(&out.stderr)
                    ),
                    Err(e) => eprintln!(
                        "   [!] failed to create build context for '{}': {}",
                        challenge.name, e
                    ),
                }
            }
        }
    }

    Ok(())
}

/// Phase 1 for a new challenge: POST core, flags, tags, topics, hints.
/// Returns `(id, has_files, has_requirements, next_challenge_name)`.
async fn create_challenge_phase1(
    client: &CtfdClient,
    challenge: &Challenge,
) -> Result<(u32, bool, bool, Option<String>)> {
    use reqwest::Method;

    let state_str = match challenge.state.as_ref().unwrap_or(&State::Visible) {
        State::Visible => "visible",
        State::Hidden => "hidden",
    };

    // Resolve the CTFd challenge type and description for instance challenges
    let (ctfd_type, description) = resolve_challenge_type_and_description(challenge, &None);

    let mut payload = json!({
        "name":        challenge.name,
        "category":    challenge.category,
        "description": description,
        "value":       challenge.value,
        "type":        ctfd_type,
        "state":       state_str,
    });
    if let Some(ref ci) = challenge.connection_info {
        payload["connection_info"] = json!(ci);
    }
    if let Some(attempts) = challenge.attempts {
        payload["attempts"] = json!(attempts);
    }
    // Merge scoring extra fields for Dynamic challenges (and Instance with decay scoring)
    if let Some(ref extra) = challenge.extra {
        if matches!(challenge.challenge_type, ChallengeType::Dynamic)
            || (challenge.challenge_type == ChallengeType::Instance && extra.initial.is_some())
        {
            merge_extra_fields(&mut payload, extra);
        }
    }
    // Include instance config fields so the CTFd plugin can store them
    if challenge.challenge_type == ChallengeType::Instance {
        merge_instance_fields(&mut payload, &challenge.instance);
    }

    let created: Challenge = client
        .execute::<Challenge, _>(Method::POST, "/challenges", Some(&payload))
        .await?
        .ok_or_else(|| anyhow!("No response body from challenge creation"))?;
    let id = created
        .id
        .ok_or_else(|| anyhow!("Created challenge returned no ID"))?;

    // Post sub-resources; on any error delete the partially-created challenge
    // so CTFd is never left with a broken entry.
    let sub_result: Result<()> = async {
        post_flags(client, id, &challenge.flags).await?;
        post_tags(client, id, &challenge.tags).await?;
        post_topics(client, id, &challenge.topics).await?;
        post_hints(client, id, &challenge.hints).await?;
        Ok(())
    }.await;
    if let Err(e) = sub_result {
        let _ = client.delete_challenge(id).await;
        return Err(e);
    }

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
    let state_str = match challenge.state.as_ref().unwrap_or(&State::Visible) {
        State::Visible => "visible",
        State::Hidden => "hidden",
    };

    let (_ctfd_type, description) = resolve_challenge_type_and_description(challenge, &None);

    let mut payload = json!({
        "name":        challenge.name,
        "category":    challenge.category,
        "description": description,
        "value":       challenge.value,
        "state":       state_str,
    });
    if let Some(ref ci) = challenge.connection_info {
        payload["connection_info"] = json!(ci);
    }
    if let Some(attempts) = challenge.attempts {
        payload["attempts"] = json!(attempts);
    }
    if let Some(ref extra) = challenge.extra {
        if matches!(challenge.challenge_type, ChallengeType::Dynamic)
            || (challenge.challenge_type == ChallengeType::Instance && extra.initial.is_some())
        {
            merge_extra_fields(&mut payload, extra);
        }
    }
    if challenge.challenge_type == ChallengeType::Instance {
        merge_instance_fields(&mut payload, &challenge.instance);
    }
    client.update_challenge(challenge_id, &payload).await?;

    // Replace flags (best-effort: skip if CTFd sub-endpoints are unavailable,
    // e.g. when Challenge Visibility is set to Private in CTFd Admin → Config)
    replace_flags(client, challenge_id, &challenge.flags).await?;
    replace_tags(client, challenge_id, &challenge.tags).await?;
    post_topics(client, challenge_id, &challenge.topics).await?;
    replace_hints(client, challenge_id, &challenge.hints).await?;
    let has_files = sync_files(client, challenge_id, &challenge.files).await;
    let has_reqs = challenge.requirements.is_some();
    Ok((has_files, has_reqs, challenge.next.clone()))
}

// ── Sub-resource replace helpers ──────────────────────────────────────────────

/// Returns true if the error looks like a non-JSON / HTML response (login redirect,
/// CTFd Private-mode catch-all, or missing endpoint).
fn is_html_or_redirect(e: &anyhow::Error) -> bool {
    let msg = e.to_string();
    msg.contains("<!DOCTYPE") || msg.contains("/login") || msg.contains("Redirecting")
        || msg.contains("JSON parse error")
}

async fn replace_flags(
    client: &CtfdClient,
    challenge_id: u32,
    flags: &Option<Vec<FlagContent>>,
) -> Result<()> {
    match client.get_challenge_flags_endpoint(challenge_id).await {
        Ok(Some(existing)) => {
            let arr = existing.as_array().cloned().unwrap_or_default();
            // Compare remote vs local content — skip if already identical
            let mut remote: Vec<String> = arr.iter()
                .filter_map(|f| f.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .collect();
            remote.sort();
            let mut local: Vec<String> = flags.as_ref().map(|fs| fs.iter().map(|f| match f {
                FlagContent::Simple(s) => s.clone(),
                FlagContent::Detailed { content, .. } => content.clone(),
            }).collect()).unwrap_or_default();
            local.sort();
            if remote == local {
                return Ok(());
            }
            for flag in &arr {
                if let Some(id) = flag.get("id").and_then(|v| v.as_u64()) {
                    client.delete_flag(id as u32).await?;
                }
            }
            post_flags(client, challenge_id, flags).await?;
        }
        Ok(None) => post_flags(client, challenge_id, flags).await?,
        Err(e) if is_html_or_redirect(&e) => {
            eprintln!(
                "  [!] challenge {}: flags endpoint unavailable (CTFd Private mode?) -- \
                 flags not updated. set Visibility to Public in CTFd Admin -> Config.",
                challenge_id
            );
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

async fn replace_tags(
    client: &CtfdClient,
    challenge_id: u32,
    tags: &Option<Vec<Tag>>,
) -> Result<()> {
    match client.get_challenge_tags_endpoint(challenge_id).await {
        Ok(Some(existing)) => {
            let arr = existing.as_array().cloned().unwrap_or_default();
            let mut remote: Vec<String> = arr.iter()
                .filter_map(|t| t.get("value").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .collect();
            remote.sort();
            let mut local: Vec<String> = tags.as_ref().map(|ts| ts.iter().map(|t| match t {
                Tag::Simple(s) => s.clone(),
                Tag::Detailed { value, .. } => value.clone(),
            }).collect()).unwrap_or_default();
            local.sort();
            if remote == local {
                return Ok(());
            }
            for tag in &arr {
                if let Some(id) = tag.get("id").and_then(|v| v.as_u64()) {
                    client.delete_tag(id as u32).await?;
                }
            }
            post_tags(client, challenge_id, tags).await?;
        }
        Ok(None) => post_tags(client, challenge_id, tags).await?,
        Err(e) if is_html_or_redirect(&e) => {
            eprintln!("  [!] challenge {}: tags endpoint unavailable -- tags not updated.", challenge_id);
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

async fn replace_hints(
    client: &CtfdClient,
    challenge_id: u32,
    hints: &Option<Vec<HintContent>>,
) -> Result<()> {
    match client.get_challenge_hints_endpoint(challenge_id).await {
        Ok(Some(existing)) => {
            let arr = existing.as_array().cloned().unwrap_or_default();
            let mut remote: Vec<String> = arr.iter()
                .filter_map(|h| h.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .collect();
            remote.sort();
            let mut local: Vec<String> = hints.as_ref().map(|hs| hs.iter().map(|h| match h {
                HintContent::Simple(s) => s.clone(),
                HintContent::Detailed { content, .. } => content.clone(),
            }).collect()).unwrap_or_default();
            local.sort();
            if remote == local {
                return Ok(());
            }
            for hint in &arr {
                if let Some(id) = hint.get("id").and_then(|v| v.as_u64()) {
                    client.delete_hint(id as u32).await?;
                }
            }
            post_hints(client, challenge_id, hints).await?;
        }
        Ok(None) => post_hints(client, challenge_id, hints).await?,
        Err(e) if is_html_or_redirect(&e) => {
            eprintln!("  [!] challenge {}: hints endpoint unavailable -- hints not updated.", challenge_id);
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

/// Compare remote filenames with local file list. If identical, return false (no re-upload needed).
/// If different, delete remote files and return true so phase 2 re-uploads the local set.
/// On endpoint error (CTFd Private mode), prints a warning and returns false to avoid duplicates.
async fn sync_files(
    client: &CtfdClient,
    challenge_id: u32,
    local_files: &Option<Vec<String>>,
) -> bool {
    let mut local_names: Vec<String> = local_files.as_ref()
        .map(|fs| fs.iter()
            .map(|f| std::path::Path::new(f)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(f.as_str())
                .to_string())
            .collect())
        .unwrap_or_default();
    local_names.sort();

    if local_names.is_empty() {
        return false;
    }

    match client.get_challenge_files_endpoint(challenge_id).await {
        Ok(Some(existing)) => {
            let arr = existing.as_array().cloned().unwrap_or_default();
            let mut remote_names: Vec<String> = arr.iter()
                .filter_map(|f| f.get("location").and_then(|v| v.as_str())
                    .and_then(|loc| loc.split('/').next_back())
                    .map(|s| s.to_string()))
                .collect();
            remote_names.sort();
            if remote_names == local_names {
                return false; // already in sync
            }
            // Delete remote files before phase 2 re-uploads
            for file in &arr {
                if let Some(id) = file.get("id").and_then(|v| v.as_u64()) {
                    let _ = client.delete_file(id as u32).await;
                }
            }
            true
        }
        Ok(None) => true, // no remote files yet, upload local ones
        Err(e) if is_html_or_redirect(&e) => {
            eprintln!("  [!] challenge {}: files endpoint unavailable -- files not synced.", challenge_id);
            false // don't re-upload blindly if we can't delete first
        }
        Err(_) => false,
    }
}

async fn post_flags(
    client: &CtfdClient,
    challenge_id: u32,
    flags: &Option<Vec<FlagContent>>,
) -> Result<()> {
    use reqwest::Method;
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
        println!("note: no challenge files found");
        return Ok(());
    }

    println!("found {} challenges:", challenges.len());
    for challenge in challenges {
        if detailed {
            match challenge.to_yaml_string() {
                Ok(yaml) => println!("{}", yaml),
                Err(e) => println!("Failed to serialize: {}", e),
            }
        } else {
            println!(
                "  - {} ({}) - {} pts",
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
        println!("note: no challenge files found");
        return Ok(());
    }

    if detailed {
        println!("\n{} challenges:", challenges.len());
        for c in &challenges {
            println!("  - {} ({}) - {} pts", c.name, c.category, c.value);
            if let Some(flags) = &c.flags {
                println!("    Flags: {}", flags.len());
            }
            if let Some(hints) = &c.hints {
                println!("    Hints: {}", hints.len());
            }
        }
    } else {
        println!("found {} challenges", challenges.len());
    }

    let stats = scanner.get_stats(&challenges);
    stats.print();
    Ok(())
}
