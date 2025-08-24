use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use nervctf::{
    challenge_manager::ChallengeManager,
    ctfd_api::{
        models::{Challenge, FlagContent},
        CtfdClient,
    },
    directory_scanner::DirectoryScanner,
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

    /// Auto-manager: automatically verify and synchronize challenges
    AutoManager {
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

    // Get environment variables
    let ctfd_url =
        env::var("CTFD_URL").map_err(|_| anyhow!("CTFD_URL environment variable is required"))?;
    let api_key = env::var("CTFD_API_KEY")
        .map_err(|_| anyhow!("CTFD_API_KEY environment variable is required"))?;

    if cli.verbose {
        println!("✅ Connected to CTFd instance at {}", ctfd_url);
        println!("📁 Base directory: {}", cli.base_dir.display());
    }

    // Initialize CTFd client
    let client = CtfdClient::new(&ctfd_url, &api_key)?;

    // Create directory scanner
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

        Commands::AutoManager { dry_run, watch } => {
            println!("🤖 Auto-manager started for {}", cli.base_dir.display());
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

/// Deploy a single challenge to CTFd with all associated components
async fn deploy_single_challenge(client: &CtfdClient, challenge: &Challenge) -> Result<u32> {
    use reqwest::Method;
    use serde_json::json;

    // Prepare challenge data
    let challenge_data = json!({
        "name": challenge.name,
        "category": challenge.category,
        "description": challenge.description,
        "value": challenge.value,
        "type": challenge.challenge_type,
        "state": challenge.state,
        "connection_info": challenge.connection_info,
        "requirements": challenge.requirements,
    });

    // Create the challenge
    let created_challenge: Challenge = client
        .execute::<Challenge, _>(Method::POST, "/challenges", Some(&challenge_data))
        .await?
        .unwrap();

    let challenge_id = created_challenge
        .id
        .ok_or_else(|| anyhow!("Challenge created but no ID returned"))?;

    println!("   📝 Created challenge with ID: {}", challenge_id);

    // Deploy flags
    if let Some(flags) = &challenge.flags {
        println!("   🚩 Deploying {} flags", flags.len());
        for flag in flags {
            match flag {
                FlagContent::Simple(content) => {
                    let flag_data = json!({
                        "challenge_id": challenge_id,
                        "content": content,
                        "type": "static",
                    });

                    client
                        .execute::<serde_json::Value, _>(Method::POST, "/flags", Some(&flag_data))
                        .await?;
                }
                FlagContent::Detailed {
                    id: _,
                    challenge_id: _,
                    type_,
                    content,
                    data,
                } => {
                    let flag_data = json!({
                        "challenge_id": challenge_id,
                        "content": content,
                        "type": serde_yaml::to_string(type_).unwrap_or_else(|_| "static".to_string()),
                        "data": serde_yaml::to_string(data).unwrap_or_else(|_| "case sensitive".to_string()),
                    });

                    client
                        .execute::<serde_json::Value, _>(Method::POST, "/flags", Some(&flag_data))
                        .await?;
                }
            };
        }
    }

    // Deploy tags
    if let Some(tags) = &challenge.tags {
        println!("   🏷️  Deploying {} tags", tags.len());
        for tag in tags {
            let tag_data = json!({
                "challenge_id": challenge_id,
                "value": tag,
            });

            client
                .execute::<serde_json::Value, _>(Method::POST, "/tags", Some(&tag_data))
                .await?;
        }
    }

    // Deploy hints
    if let Some(hints) = &challenge.hints {
        println!("   💡 Deploying {} hints", hints.len());
        for hint in hints {
            let hint_data = json!({
                "challenge_id": challenge_id,
                "content": hint.content,
                "cost": hint.cost.unwrap_or(0),
            });

            client
                .execute::<serde_json::Value, _>(Method::POST, "/hints", Some(&hint_data))
                .await?;
        }
    }

    // Note: File deployment requires actual file uploads which is more complex
    // For now, we'll just log the file information
    if let Some(files) = &challenge.files {
        println!(
            "   📁 Found {} files (file upload not implemented yet)",
            files.len()
        );
        for file in files {
            println!("     - {}", file);
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

/// Auto-manager: automatically verifies and synchronizes challenges
async fn auto_manager(
    client: &CtfdClient,
    base_dir: &PathBuf,
    dry_run: bool,
    watch: bool,
) -> Result<()> {
    println!("🎯 Auto-manager mode activated");
    println!("   Dry run: {}", dry_run);
    println!("   Watch: {}", watch);

    // Create challenge manager
    let challenge_manager = ChallengeManager::new(client.clone(), base_dir);

    // Verify local challenges first
    println!("\n🔍 Verifying local challenges...");
    match verify_local_challenges(&challenge_manager) {
        Ok(()) => println!("✅ Local challenges verification passed"),
        Err(e) => {
            eprintln!("❌ Local challenges verification failed: {}", e);
        }
    }

    // Synchronize challenges
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
        // Simple watch implementation - in production you'd use a proper file watcher
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        println!("⏰ Watch period completed (30 seconds)");
    }

    println!("\n🎉 Auto-manager completed successfully!");
    Ok(())
}

/// Verify all local challenges for correctness
fn verify_local_challenges(challenge_manager: &ChallengeManager) -> Result<()> {
    let challenges = match challenge_manager.scan_local_challenges() {
        Ok(challenges) => challenges,
        Err(e) => {
            eprintln!("⚠️  Warning: Some challenges failed to scan: {}", e);
            // Return empty vector to continue with verification of valid challenges
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
        // Basic validation
        if challenge.name.trim().is_empty() {
            eprintln!("     ❌ ERROR: Challenge name cannot be empty");
            error_count += 1;
        }

        if challenge.category.trim().is_empty() {
            eprintln!("     ❌ ERROR: Challenge category cannot be empty");
            error_count += 1;
        }

        if challenge
            .description
            .clone()
            .expect("REASON")
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
                    FlagContent::Detailed {
                        id: _,
                        challenge_id: _,
                        type_,
                        content,
                        data,
                    } => {
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
            if !challenge.flags.is_none() {
                println!("    Flags: {}", challenge.flags.clone().unwrap().len());
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
