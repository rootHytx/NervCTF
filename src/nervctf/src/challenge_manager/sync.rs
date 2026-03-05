//! Challenge synchronization module for CTFd challenge management
//! Handles synchronization between local challenge files and remote CTFd instance

use crate::challenge_manager::ChallengeManager;
use crate::ctfd_api::models::{Challenge, FlagContent};
use anyhow::{anyhow, Result};

use std::collections::HashMap;

/// Returns true if the remote challenge needs to be updated to match local.
pub fn needs_update(remote: &Challenge, local: &Challenge) -> bool {
    if remote.category != local.category {
        return true;
    }
    if remote.value != local.value {
        return true;
    }
    if remote.description != local.description {
        return true;
    }
    if remote.state != local.state {
        return true;
    }
    if remote.connection_info != local.connection_info {
        return true;
    }
    if remote.attempts != local.attempts {
        return true;
    }

    let remote_extra = serde_json::to_value(&remote.extra).unwrap_or_default();
    let local_extra = serde_json::to_value(&local.extra).unwrap_or_default();
    if remote_extra != local_extra {
        return true;
    }

    // Flags, tags, hints: the CTFd list endpoint never returns these fields,
    // so remote.* is always None when called from deploy. Only compare when
    // both sides have data (e.g. after fetching per-challenge detail).
    if let (Some(rf_list), Some(lf_list)) = (&remote.flags, &local.flags) {
        let mut rf: Vec<String> = rf_list
            .iter()
            .map(|f| match f {
                FlagContent::Simple(s) => s.clone(),
                FlagContent::Detailed { content, .. } => content.clone(),
            })
            .collect();
        rf.sort();
        let mut lf: Vec<String> = lf_list
            .iter()
            .map(|f| match f {
                FlagContent::Simple(s) => s.clone(),
                FlagContent::Detailed { content, .. } => content.clone(),
            })
            .collect();
        lf.sort();
        if rf != lf {
            return true;
        }
    }

    if let (Some(rt_list), Some(lt_list)) = (&remote.tags, &local.tags) {
        let mut rt: Vec<String> = rt_list
            .iter()
            .map(|t| t.value_str().to_string())
            .collect();
        rt.sort();
        let mut lt: Vec<String> = lt_list
            .iter()
            .map(|t| t.value_str().to_string())
            .collect();
        lt.sort();
        if rt != lt {
            return true;
        }
    }

    if let (Some(rh_list), Some(lh_list)) = (&remote.hints, &local.hints) {
        let mut rh: Vec<String> = rh_list
            .iter()
            .map(|h| h.content_str().to_string())
            .collect();
        rh.sort();
        let mut lh: Vec<String> = lh_list
            .iter()
            .map(|h| h.content_str().to_string())
            .collect();
        lh.sort();
        if rh != lh {
            return true;
        }
    }

    // Requirements: detect presence change (cannot compare IDs vs names directly)
    if remote.requirements.is_some() != local.requirements.is_some() {
        return true;
    }

    false
}

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

        // Flags, tags, hints: CTFd list endpoint never returns these fields,
        // so remote.* is always None. Only compare when both sides have data.
        if let (Some(rf_list), Some(lf_list)) = (&remote.flags, &local.flags) {
            let mut remote_flags: Vec<String> = rf_list
                .iter()
                .map(|f| match f {
                    FlagContent::Simple(s) => s.clone(),
                    FlagContent::Detailed { content, .. } => content.clone(),
                })
                .collect();
            remote_flags.sort();
            let mut local_flags: Vec<String> = lf_list
                .iter()
                .map(|f| match f {
                    FlagContent::Simple(s) => s.clone(),
                    FlagContent::Detailed { content, .. } => content.clone(),
                })
                .collect();
            local_flags.sort();
            if remote_flags != local_flags {
                return Ok(true);
            }
        }

        if let (Some(rt_list), Some(lt_list)) = (&remote.tags, &local.tags) {
            let mut remote_tags: Vec<String> =
                rt_list.iter().map(|t| t.value_str().to_string()).collect();
            remote_tags.sort();
            let mut local_tags: Vec<String> =
                lt_list.iter().map(|t| t.value_str().to_string()).collect();
            local_tags.sort();
            if remote_tags != local_tags {
                return Ok(true);
            }
        }

        if let (Some(rh_list), Some(lh_list)) = (&remote.hints, &local.hints) {
            let mut remote_hints: Vec<String> =
                rh_list.iter().map(|h| h.content_str().to_string()).collect();
            remote_hints.sort();
            let mut local_hints: Vec<String> =
                lh_list.iter().map(|h| h.content_str().to_string()).collect();
            local_hints.sort();
            if remote_hints != local_hints {
                return Ok(true);
            }
        }

        // Requirements: detect presence change
        if remote.requirements.is_some() != local.requirements.is_some() {
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctfd_api::models::{
        ChallengeType, Extra, FlagContent, HintContent, State, Tag,
    };

    fn base() -> Challenge {
        Challenge {
            name: "test".to_string(),
            category: "web".to_string(),
            value: 100,
            challenge_type: ChallengeType::Standard,
            description: Some("desc".to_string()),
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
            flags: Some(vec![FlagContent::Simple("flag{x}".to_string())]),
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

    #[test]
    fn identical_challenges_no_update() {
        let c = base();
        assert!(!needs_update(&c, &c));
    }

    #[test]
    fn different_category_triggers_update() {
        let remote = base();
        let mut local = base();
        local.category = "pwn".to_string();
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn different_value_triggers_update() {
        let remote = base();
        let mut local = base();
        local.value = 200;
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn different_description_triggers_update() {
        let remote = base();
        let mut local = base();
        local.description = Some("new desc".to_string());
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn description_none_vs_some_triggers_update() {
        let remote = base();
        let mut local = base();
        local.description = None;
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn different_state_triggers_update() {
        let remote = base();
        let mut local = base();
        local.state = Some(State::Hidden);
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn different_connection_info_triggers_update() {
        let remote = base();
        let mut local = base();
        local.connection_info = Some("nc ctf.example.com 1337".to_string());
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn different_attempts_triggers_update() {
        let remote = base();
        let mut local = base();
        local.attempts = Some(5);
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn different_extra_triggers_update() {
        let remote = base();
        let mut local = base();
        local.extra = Some(Extra { initial: Some(500), decay: Some(50), minimum: Some(100) });
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn same_extra_no_update() {
        let extra = Some(Extra { initial: Some(500), decay: Some(50), minimum: Some(100) });
        let mut remote = base();
        remote.extra = extra.clone();
        let mut local = base();
        local.extra = extra;
        assert!(!needs_update(&remote, &local));
    }

    #[test]
    fn different_flag_content_triggers_update() {
        let remote = base();
        let mut local = base();
        local.flags = Some(vec![FlagContent::Simple("flag{different}".to_string())]);
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn added_flag_triggers_update() {
        let remote = base();
        let mut local = base();
        local.flags = Some(vec![
            FlagContent::Simple("flag{x}".to_string()),
            FlagContent::Simple("flag{y}".to_string()),
        ]);
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn removed_flag_triggers_update() {
        let mut remote = base();
        remote.flags = Some(vec![
            FlagContent::Simple("flag{x}".to_string()),
            FlagContent::Simple("flag{y}".to_string()),
        ]);
        let local = base(); // only flag{x}
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn flag_order_change_no_update() {
        // Comparison is order-independent (sorted)
        let mut remote = base();
        remote.flags = Some(vec![
            FlagContent::Simple("flag{a}".to_string()),
            FlagContent::Simple("flag{b}".to_string()),
        ]);
        let mut local = base();
        local.flags = Some(vec![
            FlagContent::Simple("flag{b}".to_string()),
            FlagContent::Simple("flag{a}".to_string()),
        ]);
        assert!(!needs_update(&remote, &local));
    }

    #[test]
    fn both_flags_none_no_update() {
        let mut remote = base();
        remote.flags = None;
        let mut local = base();
        local.flags = None;
        assert!(!needs_update(&remote, &local));
    }

    #[test]
    fn detailed_flag_same_content_as_simple_no_update() {
        use crate::ctfd_api::models::{FlagType};
        let mut remote = base();
        remote.flags = Some(vec![FlagContent::Detailed {
            id: None,
            challenge_id: None,
            type_: FlagType::Static,
            content: "flag{x}".to_string(),
            data: None,
        }]);
        // local has the same content as a Simple flag — needs_update only compares content
        assert!(!needs_update(&remote, &base()));
    }

    #[test]
    fn different_tag_triggers_update() {
        let mut remote = base();
        remote.tags = Some(vec![]); // remote was queried and has no tags
        let mut local = base();
        local.tags = Some(vec![Tag::Simple("web".to_string())]);
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn tag_order_change_no_update() {
        let mut remote = base();
        remote.tags = Some(vec![
            Tag::Simple("web".to_string()),
            Tag::Simple("sql".to_string()),
        ]);
        let mut local = base();
        local.tags = Some(vec![
            Tag::Simple("sql".to_string()),
            Tag::Simple("web".to_string()),
        ]);
        assert!(!needs_update(&remote, &local));
    }

    #[test]
    fn different_hint_triggers_update() {
        let mut remote = base();
        remote.hints = Some(vec![]); // remote was queried and has no hints
        let mut local = base();
        local.hints = Some(vec![HintContent::Simple("check cookies".to_string())]);
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn hint_order_change_no_update() {
        let mut remote = base();
        remote.hints = Some(vec![
            HintContent::Simple("hint a".to_string()),
            HintContent::Simple("hint b".to_string()),
        ]);
        let mut local = base();
        local.hints = Some(vec![
            HintContent::Simple("hint b".to_string()),
            HintContent::Simple("hint a".to_string()),
        ]);
        assert!(!needs_update(&remote, &local));
    }

    #[test]
    fn detailed_hint_content_compared_correctly() {
        let mut remote = base();
        remote.hints = Some(vec![HintContent::Detailed {
            content: "look closer".to_string(),
            cost: Some(50),
            title: None,
        }]);
        let mut local = base();
        local.hints = Some(vec![HintContent::Detailed {
            content: "look closer".to_string(),
            cost: Some(100), // cost differs — but needs_update only compares content
            title: None,
        }]);
        assert!(!needs_update(&remote, &local));
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
