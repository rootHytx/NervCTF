//! Main executable for the CTFd Remote Monitor

use anyhow::Result;
use remote_monitor::ctfd_api::CtfdClient;
use std::env;
use tokio;

#[tokio::main]
async fn main() -> Result<()> {
    // Get CTFd URL and API key from environment variables
    let ctfd_url = env::var("CTFD_URL").expect("CTFD_URL environment variable not set");
    let api_key = env::var("CTFD_API_KEY").expect("CTFD_API_KEY environment variable not set");

    // Initialize CTFd client
    let client = CtfdClient::new(&ctfd_url, &api_key)?;
    println!("✅ Connected to CTFd instance at {}", ctfd_url);

    // Get current user information
    let current_user = client.get_current_user().await?;
    println!(
        "\n👤 Current user: {} (ID: {})",
        current_user.name, current_user.id
    );

    // Get all challenges
    let challenges = client.get_challenges().await?;
    println!("\n🔍 Found {} challenges:", challenges.len());
    for challenge in challenges.iter().take(5) {
        println!("- {} ({} points)", challenge.name, challenge.value);
    }
    if challenges.len() > 5 {
        println!("- ... and {} more", challenges.len() - 5);
    }

    // Get scoreboard
    let scoreboard = client.get_scoreboard().await?;
    println!("\n🏆 Scoreboard (Top 10):");
    for (i, entry) in scoreboard.iter().take(10).enumerate() {
        println!("{}. {}: {} points", i + 1, entry.account_name, entry.score);
    }

    // Get statistics
    let stats = client.get_statistics().await?;
    println!("\n📊 Statistics:");
    println!("- Total solves: {}", stats.solves["total"]);
    println!("- Total fails: {}", stats.solves["fails"]);

    println!("\n🚀 Monitoring started successfully!");
    Ok(())
}
