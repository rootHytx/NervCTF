use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use nervctf::{
    challenge_manager::ChallengeManager,
    ctfd_api::{
        models::{Challenge, FlagContent, HintContent, Tag},
        CtfdClient,
    },
    directory_scanner::DirectoryScanner,
    load_config,
};
use serde_json;
use std::env;
use std::path::PathBuf;
use tokio;

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
    /// Deploy all challenges from current directory
    Deploy,

    /// List all challenges found locally
    List {
        /// Show detailed information
        #[arg(short, long)]
        detailed: bool,
    },

    /// Scan directory for challenges
    Scan {
        /// Show detailed file information
        #[arg(short, long)]
        detailed: bool,
    },

    /// Auto: automatically verify and synchronize challenges
    Auto {
        /// Show diff without applying changes
        #[arg(short, long)]
        dry_run: bool,

        /// Watch mode: continuously monitor for changes
        #[arg(short, long)]
        watch: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

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
        if let (Some(ref url), _) = (&monitor_url, &monitor_token) {
            println!("✅ Using remote monitor at {}", url);
        } else {
            println!("✅ Connected to CTFd instance at {}", ctfd_url);
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

    // When monitor_url+token are set, use the monitor as the endpoint
    let client = if let (Some(ref url), Some(ref token)) = (&monitor_url, &monitor_token) {
        CtfdClient::new(url, token)?
    } else {
        CtfdClient::new(&ctfd_url, &api_key)?
    };

    let scanner = DirectoryScanner::new();

    match cli.command {
        Commands::Deploy => {
            println!("🚀 Deploying challenges from {}", cli.base_dir.display());
            deploy_challenges(&client, &scanner, &cli.base_dir).await?;
        }

        Commands::List { detailed } => {
            println!("📋 Listing challenges from {}", cli.base_dir.display());
            list_challenges(&scanner, &cli.base_dir, detailed).await?;
        }

        Commands::Scan { detailed } => {
            println!("🔍 Scanning directory: {}", cli.base_dir.display());
            scan_challenges(&scanner, &cli.base_dir, detailed).await?;
        }

        Commands::Auto { dry_run, watch } => {
            println!("🤖 Auto started for {}", cli.base_dir.display());
            auto_manager(&client, &cli.base_dir, dry_run, watch).await?;
        }
    }

    Ok(())
}

async fn deploy_challenges(
    client: &CtfdClient,
    scanner: &DirectoryScanner,
    base_dir: &PathBuf,
) -> Result<()> {
    let challenges = scanner.scan_directory(base_dir)?;

    if challenges.is_empty() {
        println!("ℹ️  No challenge files found");
        return Ok(());
    }

    println!("📋 Found {} challenges to deploy:", challenges.len());

    let mut successful_deployments = 0;
    let mut failed_deployments = 0;

    for challenge in challenges {
        println!(
            "\n🚀 Deploying: {} ({}) - {} points",
            challenge.name, challenge.category, challenge.value
        );

        match deploy_single_challenge(client, &challenge).await {
            Ok(challenge_id) => {
                println!(
                    "   ✅ Successfully deployed challenge (ID: {})",
                    challenge_id
                );
                successful_deployments += 1;
            }
            Err(e) => {
                println!("   ❌ Failed to deploy challenge: {}", e);
                failed_deployments += 1;
            }
        }
    }

    println!("\n📊 Deployment summary:");
    println!("   Successful: {}", successful_deployments);
    println!("   Failed: {}", failed_deployments);

    if successful_deployments > 0 {
        println!("✅ Deployment completed!");
    } else if failed_deployments > 0 {
        return Err(anyhow!("Deployment failed for all challenges"));
    }

    Ok(())
}

/// Gap 4 fix: Deploy a single challenge with all spec fields handled in order
async fn deploy_single_challenge(client: &CtfdClient, challenge: &Challenge) -> Result<u32> {
    use reqwest::Method;
    use serde_json::json;

    // Step 1: POST challenge core
    let challenge_data = json!({
        "name": challenge.name,
        "category": challenge.category,
        "description": challenge.description.as_deref().unwrap_or(""),
        "value": challenge.value,
        "type": challenge.challenge_type,
        "state": challenge.state,
        "connection_info": challenge.connection_info,
        "attempts": challenge.attempts,
        "extra": challenge.extra,
    });

    let created_challenge: Challenge = client
        .execute::<Challenge, _>(Method::POST, "/challenges", Some(&challenge_data))
        .await?
        .ok_or_else(|| anyhow!("No response from challenge creation"))?;

    let challenge_id = created_challenge
        .id
        .ok_or_else(|| anyhow!("Challenge created but no ID returned"))?;

    println!("   📝 Created challenge with ID: {}", challenge_id);

    // Step 2: POST flags
    if let Some(flags) = &challenge.flags {
        println!("   🚩 Deploying {} flags", flags.len());
        for flag in flags {
            let flag_data = match flag {
                FlagContent::Simple(content) => json!({
                    "challenge_id": challenge_id,
                    "content": content,
                    "type": "static",
                    "data": "",
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
                        "content": content,
                        "type": serde_json::to_value(type_).unwrap_or_default(),
                        "data": data_val,
                    })
                }
            };
            client
                .execute::<serde_json::Value, _>(Method::POST, "/flags", Some(&flag_data))
                .await?;
        }
    }

    // Step 3: POST tags
    if let Some(tags) = &challenge.tags {
        println!("   🏷️  Deploying {} tags", tags.len());
        for tag in tags {
            let value = match tag {
                Tag::Simple(s) => s.as_str(),
                Tag::Detailed { value, .. } => value.as_str(),
            };
            let tag_data = json!({
                "challenge_id": challenge_id,
                "value": value,
            });
            client
                .execute::<serde_json::Value, _>(Method::POST, "/tags", Some(&tag_data))
                .await?;
        }
    }

    // Step 4: POST topics
    if let Some(topics) = &challenge.topics {
        println!("   🔖 Deploying {} topics", topics.len());
        for topic in topics {
            let topic_data = json!({
                "challenge_id": challenge_id,
                "value": topic,
                "type": "challenge",
            });
            client
                .execute::<serde_json::Value, _>(Method::POST, "/topics", Some(&topic_data))
                .await?;
        }
    }

    // Step 5: POST hints (handling both Simple and Detailed variants)
    if let Some(hints) = &challenge.hints {
        println!("   💡 Deploying {} hints", hints.len());
        for hint in hints {
            let (content, cost) = match hint {
                HintContent::Simple(s) => (s.as_str(), 0u32),
                HintContent::Detailed { content, cost, .. } => {
                    (content.as_str(), cost.unwrap_or(0))
                }
            };
            let hint_data = json!({
                "challenge_id": challenge_id,
                "content": content,
                "cost": cost,
            });
            client
                .execute::<serde_json::Value, _>(Method::POST, "/hints", Some(&hint_data))
                .await?;
        }
    }

    // Step 6: POST files via multipart
    if let Some(files) = &challenge.files {
        println!("   📁 Uploading {} files", files.len());
        for file in files {
            let file_path = std::path::Path::new(&challenge.source_path).join(file);
            if file_path.exists() {
                let form = reqwest::blocking::multipart::Form::new()
                    .text("challenge_id", challenge_id.to_string())
                    .text("type", "challenge")
                    .file("file", &file_path)?;
                client
                    .post_file::<serde_json::Value>("/files", Some(form))
                    .await?;
                println!("     ✅ Uploaded: {}", file);
            } else {
                println!("     ⚠️  File not found: {}", file_path.display());
            }
        }
    }

    // Step 7: PATCH requirements — resolve names→IDs
    if let Some(requirements) = &challenge.requirements {
        let prereq_names = requirements.prerequisite_names();
        if !prereq_names.is_empty() {
            println!("   🔗 Setting {} requirements", prereq_names.len());
            let all_challenges: Vec<Challenge> = client
                .execute::<Vec<Challenge>, _>(Method::GET, "/challenges", None::<&()>)
                .await?
                .unwrap_or_default();

            let mut prereq_ids: Vec<u32> = Vec::new();
            for req in &prereq_names {
                if let Ok(id) = req.parse::<u32>() {
                    prereq_ids.push(id);
                } else if let Some(found) = all_challenges.iter().find(|c| c.name == *req) {
                    if let Some(id) = found.id {
                        prereq_ids.push(id);
                    }
                }
            }

            if !prereq_ids.is_empty() {
                let req_data = json!({
                    "requirements": {
                        "prerequisites": prereq_ids,
                    }
                });
                client
                    .execute::<serde_json::Value, _>(
                        Method::PATCH,
                        &format!("/challenges/{}", challenge_id),
                        Some(&req_data),
                    )
                    .await?;
            }
        }
    }

    // Step 8: PATCH next if set
    if let Some(next_name) = &challenge.next {
        let all_challenges: Vec<Challenge> = client
            .execute::<Vec<Challenge>, _>(Method::GET, "/challenges", None::<&()>)
            .await?
            .unwrap_or_default();

        if let Some(next_challenge) = all_challenges.iter().find(|c| c.name == *next_name) {
            if let Some(next_id) = next_challenge.id {
                let next_data = json!({ "next_id": next_id });
                client
                    .execute::<serde_json::Value, _>(
                        Method::PATCH,
                        &format!("/challenges/{}", challenge_id),
                        Some(&next_data),
                    )
                    .await?;
            }
        }
    }

    Ok(challenge_id)
}

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
                Err(e) => println!("Failed to serialize challenge to YAML: {}", e),
            }
        } else {
            println!(
                "  - {} ({}) - {} points",
                challenge.name, challenge.category, challenge.value
            );
        }
    }

    Ok(())
}

/// Auto: automatically verifies and synchronizes challenges
async fn auto_manager(
    client: &CtfdClient,
    base_dir: &PathBuf,
    dry_run: bool,
    watch: bool,
) -> Result<()> {
    println!("🎯 Auto mode activated");
    println!("   Dry run: {}", dry_run);
    println!("   Watch: {}", watch);

    let challenge_manager = ChallengeManager::new(client.clone(), base_dir);

    println!("\n🔍 Verifying local challenges...");
    match verify_local_challenges(&challenge_manager) {
        Ok(()) => println!("✅ Local challenges verification passed"),
        Err(e) => {
            eprintln!("❌ Local challenges verification failed: {}", e);
        }
    }

    println!("\n🔄 Synchronizing challenges...");
    let mut synchronizer = challenge_manager.synchronizer();

    if dry_run {
        println!("📋 Dry run mode - showing diff only");
        synchronizer.sync(false).await?;
        println!("✅ Dry run completed - no changes were made");
    } else {
        println!("🚀 Applying changes to CTFd instance");
        synchronizer.sync(true).await?;
        println!("✅ Synchronization completed successfully!");
    }

    if watch {
        println!("\n👀 Watch mode enabled - monitoring for changes...");
        println!("   Press Ctrl+C to stop watching");
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        println!("⏰ Watch period completed (30 seconds)");
    }

    println!("\n🎉 Auto completed successfully!");
    Ok(())
}

/// Bug 3 fix: verify_local_challenges no longer panics on missing description
fn verify_local_challenges(challenge_manager: &ChallengeManager) -> Result<()> {
    let challenges = match challenge_manager.scan_local_challenges() {
        Ok(challenges) => challenges,
        Err(e) => {
            eprintln!("⚠️  Warning: Some challenges failed to scan: {}", e);
            Vec::new()
        }
    };

    if challenges.is_empty() {
        return Err(anyhow!("No local challenges found"));
    }

    println!("📋 Found {} local challenges to verify:", challenges.len());

    let mut error_count = 0;
    let mut warning_count = 0;

    for challenge in &challenges {
        if challenge.name.trim().is_empty() {
            eprintln!("     ❌ ERROR: Challenge name cannot be empty");
            error_count += 1;
        }

        if challenge.category.trim().is_empty() {
            eprintln!("     ❌ ERROR: Challenge category cannot be empty");
            error_count += 1;
        }

        // Bug 3 fix: use as_deref().unwrap_or("") instead of .expect()
        if challenge
            .description
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            eprintln!("     ⚠️  WARNING: Challenge description is empty");
            warning_count += 1;
        }

        if challenge.value == 0 {
            eprintln!("     ⚠️  WARNING: Challenge value is zero");
            warning_count += 1;
        }

        if challenge.flags.is_none() {
            eprintln!("     ❌ ERROR: Challenge must have at least one flag");
            error_count += 1;
        }

        // Validate flags
        for (i, flags) in challenge.flags.iter().enumerate() {
            for flag in flags {
                match flag {
                    FlagContent::Simple(content) => {
                        if content.is_empty() {
                            eprintln!("     ❌ ERROR: Flag {} content cannot be empty", i + 1);
                            error_count += 1;
                        }
                    }
                    FlagContent::Detailed { content, .. } => {
                        if content.is_empty() {
                            eprintln!("     ❌ ERROR: Flag {} content cannot be empty", i + 1);
                            error_count += 1;
                        }
                    }
                };
            }
        }

        // Validate files exist if specified
        if let Some(files) = &challenge.files {
            for file in files {
                let file_path = PathBuf::from(format!(
                    "{}/{}",
                    challenge.source_path.clone(),
                    file.clone()
                ));
                if !file_path.exists() {
                    eprintln!(
                        "     ❌ ERROR: Referenced file does not exist: {}",
                        file_path.display()
                    );
                    error_count += 1;
                }
            }
        }
    }

    println!("\n📊 Verification summary:");
    println!("   Total challenges: {}", challenges.len());
    println!("   Errors: {}", error_count);
    println!("   Warnings: {}", warning_count);

    if error_count > 0 {
        return Err(anyhow!("Verification failed with {} error(s)", error_count));
    }

    if warning_count > 0 {
        println!(
            "⚠️  Verification completed with {} warning(s)",
            warning_count
        );
    } else {
        println!("✅ All challenges passed verification");
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
        println!("\n📋 Found {} challenges:", challenges.len());
        for challenge in &challenges {
            println!(
                "  - {} ({}) - {} points",
                challenge.name, challenge.category, challenge.value
            );
            if challenge.flags.is_some() {
                println!("    Flags: {}", challenge.flags.as_ref().unwrap().len());
            }
            if let Some(hints) = &challenge.hints {
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
