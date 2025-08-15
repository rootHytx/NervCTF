use anyhow::{Context, Result};
use clap::Subcommand;
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use walkdir::WalkDir;

/// Recursive CTF challenge manager
pub struct ChallengeManager {
    root_path: PathBuf,
}

#[derive(Subcommand, Debug)]
pub enum ChallengeOperation {
    /// Sync challenge with CTFd
    Sync,
    /// Install challenge dependencies
    Install,
    /// Lint challenge configuration
    Lint,
    /// Verify challenge setup
    Verify,
    /// Deploy challenge
    Deploy,
    /// Push challenge to CTFd
    Push,
}

impl ChallengeManager {
    /// Create a new ChallengeManager for the given root path
    pub fn new<P: AsRef<Path>>(root_path: P) -> Self {
        ChallengeManager {
            root_path: root_path.as_ref().to_path_buf(),
        }
    }

    /// Execute an operation on all challenges recursively
    pub fn execute_operation(&self, operation: ChallengeOperation) -> Result<()> {
        let challenges = self.find_challenges()?;
        let total = challenges.len();
        let pb = ProgressBar::new(total as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
                .unwrap()
                .progress_chars("##-"),
        );

        let mut errors = Vec::new();

        for (i, challenge_path) in challenges.into_iter().enumerate() {
            pb.set_position(i as u64);
            pb.set_message(format!("Processing: {}", challenge_path.display()));

            let op_str = match operation {
                ChallengeOperation::Sync => "sync",
                ChallengeOperation::Install => "install",
                ChallengeOperation::Lint => "lint",
                ChallengeOperation::Verify => "verify",
                ChallengeOperation::Deploy => "deploy",
                ChallengeOperation::Push => "push",
            };

            let output = Command::new("ctfcli")
                .arg(op_str)
                .arg("--challenge")
                .arg(&challenge_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .current_dir(self.root_path.parent().unwrap_or(&self.root_path))
                .output()
                .with_context(|| {
                    format!(
                        "Failed to execute ctfcli {} on {}",
                        op_str,
                        challenge_path.display()
                    )
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                errors.push((
                    challenge_path,
                    format!(
                        "Operation '{}' failed for {}: {}",
                        op_str,
                        challenge_path.display(),
                        stderr
                    ),
                ));
            }
        }

        pb.finish_with_message("Operation completed");

        if !errors.is_empty() {
            eprintln!("\nErrors encountered:");
            for (path, error) in errors {
                eprintln!("- {}: {}", path.display(), error);
            }
            anyhow::bail!("{} challenges failed to process", errors.len());
        }

        println!("Successfully processed all {} challenges", total);
        Ok(())
    }

    /// Find all challenge.yml files recursively
    fn find_challenges(&self) -> Result<Vec<PathBuf>> {
        let mut challenges = Vec::new();

        for entry in WalkDir::new(&self.root_path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() && path.file_name().and_then(|s| s.to_str()) == Some("challenge.yml")
            {
                challenges.push(path.to_path_buf());
            }
        }

        if challenges.is_empty() {
            anyhow::bail!(
                "No challenge.yml files found in {}",
                self.root_path.display()
            );
        }

        Ok(challenges)
    }
}
