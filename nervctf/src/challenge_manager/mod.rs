//! CTFd Challenge Manager
//! Provides comprehensive challenge management functionality including CRUD operations,
//! synchronization, and local file system management for CTFd challenges.

use crate::ctfd_api::models::{Challenge, FlagContent, HintContent, Tag};
use crate::ctfd_api::{CtfdClient, RequirementsQueue};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub mod sync;

/// Main challenge manager for CTFd challenge operations
#[derive(Clone)]
pub struct ChallengeManager {
    client: CtfdClient,
    base_path: PathBuf,
    requirements_queue: RequirementsQueue,
}

impl ChallengeManager {
    /// Creates a new ChallengeManager instance
    pub fn new(client: CtfdClient, base_path: &Path) -> Self {
        Self {
            client,
            base_path: base_path.to_path_buf(),
            requirements_queue: RequirementsQueue::new(),
        }
    }

    /// Get all challenges from the remote CTFd instance
    pub async fn get_all_challenges(&self) -> Result<Option<Vec<Challenge>>> {
        self.client.get_challenges().await
    }

    /// Get a specific challenge by ID
    pub async fn get_challenge(&self, id: u32) -> Result<Option<Challenge>> {
        self.client.get_challenge(id).await
    }

    /// Get a challenge by name
    pub async fn get_challenge_by_name(&self, name: &str) -> Result<Option<Challenge>> {
        let challenges = self.get_all_challenges().await?.unwrap();
        Ok(Option::from(
            challenges.into_iter().find(|c| c.name == name),
        ))
    }

    /// Get the base path for challenge files
    pub fn get_base_path(&self) -> &Path {
        &self.base_path
    }

    pub fn generate_requirements_list(&mut self, challenges: Vec<Challenge>) {
        for chall in challenges {
            if let Some(reqs) = &chall.requirements {
                self.requirements_queue
                    .add(chall.name.clone(), reqs.prerequisite_names());
            }
        }
    }

    /// Create a new challenge from a configuration object
    pub async fn create_challenge(&self, config: &Challenge) -> Result<Option<Challenge>> {
        let challenge_data = json!({
            "name": config.name,
            "category": config.category,
            "description": config.description,
            "type": config.challenge_type,
            "value": config.value,
        });
        let challenge = self.client.create_challenge(&challenge_data).await?;

        Ok(challenge)
    }

    /// Update an existing challenge
    pub async fn update_challenge(&self, id: u32, config: &Challenge) -> Result<Option<Challenge>> {
        // Update: challenge core
        let challenge_data = json!({
            "name": config.name,
            "category": config.category,
            "description": config.description,
            "type": config.challenge_type,
            "value": config.value,
        });
        let challenge = self.client.update_challenge(id, &challenge_data).await?;
        let challenge_id = self
            .get_all_challenges()
            .await?
            .unwrap()
            .iter()
            .find(|c| c.name == config.name)
            .and_then(|c| c.id)
            .unwrap();

        // Update: flags
        let installed_flags = self
            .client
            .get_challenge_flags_endpoint(challenge_id)
            .await?
            .unwrap();
        for flag in installed_flags.as_array().unwrap() {
            if let Some(flag_id) = flag.get("id").and_then(Value::as_u64) {
                self.client.delete_flag(flag_id as u32).await?;
            }
        }
        if let Some(flags) = &config.flags {
            for flag in flags {
                match flag {
                    FlagContent::Simple(content) => {
                        let flag_data = json!({
                            "content": content,
                            "type": "static",
                            "data": "",
                            "challenge_id": challenge_id,
                        });
                        self.client.create_flag(&flag_data).await?;
                    }
                    FlagContent::Detailed {
                        id: _,
                        challenge_id: _,
                        type_,
                        content,
                        data,
                    } => {
                        let flag_data = json!({
                            "content": content,
                            "type": format!("{:?}", type_).to_lowercase(),
                            "data": data.as_ref().map(|d| format!("{:?}", d).to_lowercase()).unwrap_or_default(),
                            "challenge_id": challenge_id,
                        });
                        self.client.create_flag(&flag_data).await?;
                    }
                }
            }
        };

        // Update: tags
        let installed_tags = self
            .client
            .get_challenge_tags_endpoint(challenge_id)
            .await?
            .unwrap();
        for tag in installed_tags.as_array().unwrap() {
            if let Some(tag_id) = tag.get("id").and_then(Value::as_u64) {
                self.client.delete_tag(tag_id as u32).await?;
            }
        }
        if let Some(tags) = &config.tags {
            for tag in tags {
                match tag {
                    Tag::Simple(content) => {
                        let tag_data = json!({
                            "value": content,
                            "challenge_id": challenge_id,
                        });
                        self.client.create_tag(&tag_data).await?;
                    }
                    Tag::Detailed {
                        challenge_id: _,
                        id: _,
                        value,
                    } => {
                        let tag_data = json!({
                            "value": value,
                            "challenge_id": challenge_id,
                        });
                        self.client.create_tag(&tag_data).await?;
                    }
                }
            }
        };

        // Update: files
        let installed_files = self
            .client
            .get_challenge_files_endpoint(challenge_id)
            .await?
            .unwrap();
        for file in installed_files.as_array().unwrap() {
            if let Some(file_id) = file.get("id").and_then(Value::as_u64) {
                self.client.delete_file(file_id as u32).await?;
            }
        }
        if let Some(files) = &config.files {
            for file in files {
                let file_path = Path::new(config.source_path.as_str()).join(&file.clone());
                let form = reqwest::blocking::multipart::Form::new()
                    .text("challenge_id", challenge_id.to_string())
                    .text("type", "challenge")
                    .file("file", file_path)?;
                self.client.create_file(form).await?;
            }
        };

        // Update: hints — Bug 4 fix: "value" → "cost" + handle HintContent variants
        let installed_hints = self
            .client
            .get_challenge_hints_endpoint(challenge_id)
            .await?
            .unwrap();
        for hint in installed_hints.as_array().unwrap() {
            if let Some(hint_id) = hint.get("id").and_then(Value::as_u64) {
                self.client.delete_hint(hint_id as u32).await?;
            }
        }
        if let Some(hints) = &config.hints {
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
                self.client.create_hint(&hint_data).await?;
            }
        };

        // Patch: requirements
        if let Some(requirements) = &config.requirements {
            let prereq_names = requirements.prerequisite_names();
            let mut required_challenges: Vec<u32> = Vec::new();
            let installed_challenges = self.get_all_challenges().await?.unwrap();
            let mut names = vec![];
            for i in installed_challenges.iter() {
                names.push(i.name.clone());
            }
            names.sort();
            println!("names: {:#?}", names);
            for req in &prereq_names {
                if req.trim().is_empty() {
                    continue;
                }
                // Try as integer ID first
                if let Ok(id) = req.parse::<u32>() {
                    required_challenges.push(id);
                } else {
                    let req_challenge = installed_challenges
                        .iter()
                        .find(|c| c.name == *req)
                        .ok_or_else(|| anyhow!("Required challenge '{}' not found", req))?;
                    required_challenges.push(
                        self.client
                            .get_challenge_id(&req_challenge.name)
                            .await?
                            .unwrap(),
                    );
                }
            }
            let req_data = json!({
                "requirements": json!({
                    "prerequisites": required_challenges,
                }),
            });
            self.client
                .update_challenge(challenge_id, &req_data)
                .await?;
        };

        // Patch: state
        if let Some(state) = &config.state {
            let state_data = json!({
                "state": format!("{:?}", state).to_lowercase(),
            });
            self.client
                .update_challenge(challenge_id, &state_data)
                .await?;
        };

        Ok(Option::from(challenge))
    }

    /// Delete a challenge by ID
    pub async fn delete_challenge(&self, id: u32) -> Result<()> {
        self.client.delete_challenge(id).await
    }

    /// Create flags for a challenge
    pub async fn create_flag(
        &self,
        challenge_id: u32,
        flags: Vec<FlagContent>,
    ) -> Result<Option<Value>> {
        let flag_data = serde_json::json!(flags
            .iter()
            .map(|flag| {
                match flag {
                    FlagContent::Simple(content) => serde_json::json!({
                        "challenge_id": challenge_id,
                        "content": content,
                        "type": "static",
                        "data": "",
                    }),
                    FlagContent::Detailed {
                        id: _,
                        challenge_id: _,
                        type_,
                        content,
                        data,
                    } => serde_json::json!({
                        "challenge_id": challenge_id,
                        "content": content,
                        "type": format!("{:?}", type_).to_lowercase(),
                        "data": data.as_ref().map(|d| format!("{:?}", d).to_lowercase()).unwrap_or_default(),
                    }),
                }
            })
            .collect::<Vec<_>>());
        self.client
            .execute(reqwest::Method::POST, "/flags", Some(&flag_data))
            .await
    }

    /// Scan local challenges from the file system
    pub fn scan_local_challenges(&self) -> Result<Vec<Challenge>> {
        let mut challenges = Vec::new();
        let challenges_dir = self.base_path.join("challenges");

        if !challenges_dir.exists() {
            return Err(anyhow!(
                "Challenges directory not found at {}",
                challenges_dir.display()
            ));
        }

        for category_entry in fs::read_dir(&challenges_dir)? {
            let category_path = category_entry?.path();
            if category_path.is_dir() {
                for challenge_entry in WalkDir::new(&category_path).max_depth(1).min_depth(1) {
                    let challenge_entry = challenge_entry?;
                    let challenge_path = challenge_entry.path().to_path_buf();
                    if challenge_path.is_dir() {
                        let yml_path = challenge_path.join("challenge.yml");
                        if yml_path.exists() {
                            let yml_content = fs::read_to_string(&yml_path).with_context(|| {
                                format!("Failed to read {}", yml_path.display())
                            })?;
                            match serde_yaml::from_str::<Challenge>(&yml_content) {
                                Ok(mut config) => {
                                    config.source_path =
                                        challenge_path.clone().display().to_string();
                                    challenges.push(config);
                                }
                                Err(e) => {
                                    eprintln!("❌ Failed to parse {}: {}", yml_path.display(), e);
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(challenges)
    }

    /// Get a local challenge by name
    pub fn get_local_challenge(&self, name: &str) -> Result<Option<Challenge>> {
        match self.scan_local_challenges() {
            Ok(challenges) => Ok(challenges.into_iter().find(|c| c.name == name)),
            Err(e) => {
                eprintln!("⚠️  Warning: Some challenges failed to scan: {}", e);
                Ok(None)
            }
        }
    }

    /// Create a new challenge from a YAML file
    pub async fn create_challenge_from_file(&self, yaml_path: &Path) -> Result<Option<Challenge>> {
        let yml_content = fs::read_to_string(yaml_path)
            .with_context(|| format!("Failed to read {}", yaml_path.display()))?;

        let config: Challenge = serde_yaml::from_str(&yml_content)
            .with_context(|| format!("Failed to parse {}", yaml_path.display()))?;

        self.create_challenge(&config).await
    }

    /// Update a challenge from a YAML file
    pub async fn update_challenge_from_file(
        &self,
        challenge_id: u32,
        yaml_path: &Path,
    ) -> Result<Option<Challenge>> {
        let yml_content = fs::read_to_string(yaml_path)
            .with_context(|| format!("Failed to read {}", yaml_path.display()))?;

        let config: Challenge = serde_yaml::from_str(&yml_content)
            .with_context(|| format!("Failed to parse {}", yaml_path.display()))?;

        self.update_challenge(challenge_id, &config).await
    }

    /// Export challenges to a directory structure
    pub async fn export_challenges(&self, export_path: &Path) -> Result<()> {
        let challenges = self.get_all_challenges().await?.unwrap();

        for challenge in challenges {
            let (name, category) = (challenge.name.clone(), challenge.category.clone());
            let category_dir = export_path.join(&category);
            let challenge_dir = category_dir.join(&name);

            fs::create_dir_all(&challenge_dir)?;

            let yaml_content = serde_yaml::to_string(&challenge)?;
            fs::write(challenge_dir.join("challenge.yml"), yaml_content)?;
        }

        Ok(())
    }

    /// Create a synchronization instance
    pub fn synchronizer(&self) -> sync::ChallengeSynchronizer {
        sync::ChallengeSynchronizer::new(self.clone())
    }
}

/// Utility functions for challenge management
pub mod utils {
    use super::*;

    /// Validate a challenge configuration
    pub fn validate_challenge_config(config: &Challenge) -> Result<()> {
        if config.name.trim().is_empty() {
            return Err(anyhow!("Challenge name cannot be empty"));
        }

        if config.category.trim().is_empty() {
            return Err(anyhow!("Challenge category cannot be empty"));
        }

        if config.value == 0 {
            return Err(anyhow!("Challenge value cannot be zero"));
        }

        if config.flags.is_none() || config.flags.as_ref().unwrap().is_empty() {
            return Err(anyhow!("Challenge must have at least one flag"));
        }

        Ok(())
    }
}
