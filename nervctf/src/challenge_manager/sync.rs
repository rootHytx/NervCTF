//! Challenge synchronization module for CTFd challenge management
//! Handles synchronization between local challenge files and remote CTFd instance

use crate::challenge_manager::ChallengeManager;
use crate::ctfd_api::models::{Challenge, FlagContent};
use anyhow::{anyhow, Result};

use std::collections::HashMap;

/// Synchronizes challenges between local files and remote CTFd instance
pub struct ChallengeSynchronizer {
    challenge_manager: ChallengeManager,
}

impl ChallengeSynchronizer {
    /// Creates a new ChallengeSynchronizer instance
    pub fn new(challenge_manager: ChallengeManager) -> Self {
        Self { challenge_manager }
    }

    /// Synchronizes challenges between local and remote
    pub async fn sync(&mut self, show_diff: bool) -> Result<()> {
        println!("🔄 Starting challenge synchronization...");

        let local_challenges = self.challenge_manager.scan_local_challenges()?;
        println!("📊 Local challenges: {}", local_challenges.len());
        let remote_challenges = self.challenge_manager.get_all_challenges().await?.unwrap();
        println!("📊 Remote challenges: {}", remote_challenges.len());

        self.challenge_manager
            .generate_requirements_list(local_challenges.clone());

        let local_map: HashMap<String, &Challenge> = local_challenges
            .iter()
            .map(|c| (c.name.clone(), c))
            .collect();

        let remote_map: HashMap<String, &crate::ctfd_api::models::Challenge> = remote_challenges
            .iter()
            .map(|c| (c.name.clone(), c))
            .collect();

        let mut actions = Vec::new();

        for (name, local_challenge) in &local_map {
            if let Some(remote_challenge) = remote_map.get(name) {
                if self.needs_update(remote_challenge, local_challenge)? {
                    actions.push(SyncAction::Update {
                        name: name.clone(),
                        local: local_challenge,
                        remote: remote_challenge,
                    });
                } else {
                    actions.push(SyncAction::UpToDate {
                        name: name.clone(),
                        challenge: local_challenge,
                    });
                }
            } else {
                actions.push(SyncAction::Create {
                    name: name.clone(),
                    challenge: local_challenge,
                });
            }
        }

        for (name, remote_challenge) in &remote_map {
            if !local_map.contains_key(name) {
                actions.push(SyncAction::RemoteOnly {
                    name: name.clone(),
                    challenge: remote_challenge,
                });
            }
        }

        if show_diff {
            self.show_diff(&actions)?;
        }

        self.execute_actions(actions).await?;

        println!("✅ Synchronization completed!");
        Ok(())
    }

    /// Gap 5 fix: comprehensive field comparison
    fn needs_update(
        &self,
        remote: &crate::ctfd_api::models::Challenge,
        local: &Challenge,
    ) -> Result<bool> {
        if remote.category != local.category {
            return Ok(true);
        }
        if remote.value != local.value {
            return Ok(true);
        }
        if remote.description != local.description {
            return Ok(true);
        }
        if remote.state != local.state {
            return Ok(true);
        }
        if remote.connection_info != local.connection_info {
            return Ok(true);
        }
        if remote.attempts != local.attempts {
            return Ok(true);
        }

        // Compare extra by JSON serialization
        let remote_extra = serde_json::to_value(&remote.extra).unwrap_or_default();
        let local_extra = serde_json::to_value(&local.extra).unwrap_or_default();
        if remote_extra != local_extra {
            return Ok(true);
        }

        // Compare flags (sorted content)
        let mut remote_flags: Vec<String> = remote
            .flags
            .as_ref()
            .map(|v| {
                v.iter()
                    .map(|f| match f {
                        FlagContent::Simple(s) => s.clone(),
                        FlagContent::Detailed { content, .. } => content.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        remote_flags.sort();

        let mut local_flags: Vec<String> = local
            .flags
            .as_ref()
            .map(|v| {
                v.iter()
                    .map(|f| match f {
                        FlagContent::Simple(s) => s.clone(),
                        FlagContent::Detailed { content, .. } => content.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        local_flags.sort();

        if remote_flags != local_flags {
            return Ok(true);
        }

        // Compare tags (sorted)
        let mut remote_tags: Vec<String> = remote
            .tags
            .as_ref()
            .map(|v| v.iter().map(|t| t.value_str().to_string()).collect())
            .unwrap_or_default();
        remote_tags.sort();

        let mut local_tags: Vec<String> = local
            .tags
            .as_ref()
            .map(|v| v.iter().map(|t| t.value_str().to_string()).collect())
            .unwrap_or_default();
        local_tags.sort();

        if remote_tags != local_tags {
            return Ok(true);
        }

        // Compare hints (sorted content)
        let mut remote_hints: Vec<String> = remote
            .hints
            .as_ref()
            .map(|v| v.iter().map(|h| h.content_str().to_string()).collect())
            .unwrap_or_default();
        remote_hints.sort();

        let mut local_hints: Vec<String> = local
            .hints
            .as_ref()
            .map(|v| v.iter().map(|h| h.content_str().to_string()).collect())
            .unwrap_or_default();
        local_hints.sort();

        if remote_hints != local_hints {
            return Ok(true);
        }

        Ok(false)
    }

    /// Shows the synchronization diff
    fn show_diff(&self, actions: &[SyncAction<'_>]) -> Result<()> {
        println!("\n📋 Synchronization Diff:");
        println!("{}", "=".repeat(50));
        let mut created_string = String::from("➕ CREATE:\n");
        let mut updated_string = String::from("🔄 UPDATE:\n");
        let mut up_to_date_string = String::from("✅ UP-TO-DATE:\n");
        let mut remote_only_string = String::from("ℹ️  REMOTE-ONLY:\n");
        let mut has_creates = false;
        let mut has_updates = false;
        let mut has_up_to_date = false;
        let mut has_remote_only = false;

        for action in actions {
            match action {
                SyncAction::Create { name, .. } => {
                    if !has_creates {
                        has_creates = true;
                    }
                    created_string.push_str(format!("\t - {}\n", name).as_str());
                }
                SyncAction::Update { name, .. } => {
                    if !has_updates {
                        has_updates = true;
                    }
                    updated_string.push_str(format!("\t - {}\n", name).as_str());
                }
                SyncAction::UpToDate { name, .. } => {
                    if !has_up_to_date {
                        has_up_to_date = true;
                    }
                    up_to_date_string.push_str(format!("\t - {}\n", name).as_str());
                }
                SyncAction::RemoteOnly { name, .. } => {
                    if !has_remote_only {
                        has_remote_only = true;
                    }
                    remote_only_string.push_str(format!("\t - {}\n", name).as_str());
                }
            }
        }
        if has_creates {
            println!("{}", created_string);
        }
        if has_updates {
            println!("{}", updated_string);
        }
        if has_up_to_date {
            println!("{}", up_to_date_string);
        }
        if has_remote_only {
            println!("{}", remote_only_string);
        }
        println!("{}", "=".repeat(50));
        Ok(())
    }

    /// Executes synchronization actions
    async fn execute_actions(&mut self, mut actions: Vec<SyncAction<'_>>) -> Result<()> {
        let mut created = 0;
        let mut updated = 0;
        let mut up_to_date = 0;
        let mut remote_only = 0;
        println!("Actions: {}", actions.len());
        actions = self
            .challenge_manager
            .requirements_queue
            .resolve_dependencies(actions);
        println!("Actions: {}", actions.len());
        println!("Do you wish to proceed? (y/N)");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            println!("❌ Aborting synchronization.");
            return Ok(());
        }
        println!("\n🚀 Executing synchronization actions...");
        for action in &actions {
            match action {
                SyncAction::Create { name, challenge } => {
                    println!("🆕 Creating: {}", name);
                    self.challenge_manager.create_challenge(challenge).await?;
                    created += 1;
                }
                SyncAction::Update {
                    name,
                    local,
                    remote,
                } => {
                    println!("🔄 Updating: {}", name);
                    let challenge_id = remote
                        .id
                        .ok_or_else(|| anyhow!("Remote challenge has no ID"))?;
                    self.challenge_manager
                        .update_challenge(challenge_id, local)
                        .await?;
                    updated += 1;
                }
                SyncAction::UpToDate { name, .. } => {
                    println!("✅ Up-to-date: {}", name);
                    up_to_date += 1;
                }
                SyncAction::RemoteOnly { name, .. } => {
                    println!("ℹ️  Remote-only: {}", name);
                    remote_only += 1;
                }
            }
        }
        println!("\n📊 Sync Summary:");
        println!("  Created: {}", created);
        println!("  Updated: {}", updated);
        println!("  Up-to-date: {}", up_to_date);
        println!("  Remote-only: {}", remote_only);

        Ok(())
    }
}

/// Represents synchronization actions
#[derive(Clone, Debug)]
pub enum SyncAction<'a> {
    Create {
        name: String,
        challenge: &'a Challenge,
    },
    Update {
        name: String,
        local: &'a Challenge,
        remote: &'a crate::ctfd_api::models::Challenge,
    },
    UpToDate {
        name: String,
        challenge: &'a Challenge,
    },
    RemoteOnly {
        name: String,
        challenge: &'a crate::ctfd_api::models::Challenge,
    },
}
