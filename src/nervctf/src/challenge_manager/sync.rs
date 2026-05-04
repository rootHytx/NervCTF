use crate::ctfd_api::models::{Challenge, FlagContent, HintContent};

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

    // Fix 1: Compare flags by (content, type_str, data_str) tuples
    if let (Some(rf_list), Some(lf_list)) = (&remote.flags, &local.flags) {
        let flag_key = |f: &FlagContent| -> (String, &'static str, &'static str) {
            match f {
                FlagContent::Simple(s) => (s.clone(), "static", ""),
                FlagContent::Detailed { content, type_, data, .. } => (
                    content.clone(),
                    type_.as_str(),
                    data.as_ref().map(|d| d.as_str()).unwrap_or(""),
                ),
            }
        };
        let mut rf: Vec<_> = rf_list.iter().map(flag_key).collect();
        rf.sort();
        let mut lf: Vec<_> = lf_list.iter().map(flag_key).collect();
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

    // Fix 2: Compare hints by (content, cost) tuples
    if let (Some(rh_list), Some(lh_list)) = (&remote.hints, &local.hints) {
        let hint_key = |h: &HintContent| -> (String, u32) {
            match h {
                HintContent::Simple(s) => (s.clone(), 0),
                HintContent::Detailed { content, cost, .. } => {
                    (content.clone(), cost.unwrap_or(0))
                }
            }
        };
        let mut rh: Vec<_> = rh_list.iter().map(hint_key).collect();
        rh.sort();
        let mut lh: Vec<_> = lh_list.iter().map(hint_key).collect();
        lh.sort();
        if rh != lh {
            return true;
        }
    }

    // Fix 3: Compare requirements by sorted prerequisite name vectors
    match (&remote.requirements, &local.requirements) {
        (None, None) => {}
        (Some(_), None) | (None, Some(_)) => return true,
        (Some(r), Some(l)) => {
            let mut rn = r.prerequisite_names();
            rn.sort();
            let mut ln = l.prerequisite_names();
            ln.sort();
            if rn != ln {
                return true;
            }
        }
    }

    // Fix 4: Compare instance configs by JSON value
    let remote_instance = serde_json::to_value(&remote.instance).unwrap_or_default();
    let local_instance = serde_json::to_value(&local.instance).unwrap_or_default();
    if remote_instance != local_instance {
        return true;
    }

    false
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctfd_api::models::{
        ChallengeType, Extra, FlagContent, FlagData, FlagType, HintContent, InstanceBackend,
        InstanceConfig, Requirements, State, Tag,
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
            instance: None,
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

    fn base_instance_config() -> InstanceConfig {
        InstanceConfig {
            backend: InstanceBackend::Docker,
            image: Some("nginx:latest".to_string()),
            compose_file: None,
            compose_service: None,
            lxc_image: None,
            vagrantfile: None,
            internal_port: 80,
            connection: "http://{host}:{port}".to_string(),
            timeout_minutes: Some(30),
            max_renewals: None,
            command: None,
            flag_mode: None,
            flag_prefix: None,
            flag_suffix: None,
            random_flag_length: None,
            flag_delivery: None,
            flag_file_path: None,
            flag_service: None,
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
        local.extra = Some(Extra { initial: Some(500), decay: Some(50), minimum: Some(100), ..Default::default() });
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn same_extra_no_update() {
        let extra = Some(Extra { initial: Some(500), decay: Some(50), minimum: Some(100), ..Default::default() });
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

    // Fix 1: detailed flag with same content but different type triggers update
    #[test]
    fn flag_type_change_triggers_update() {
        let mut remote = base();
        remote.flags = Some(vec![FlagContent::Detailed {
            id: None,
            challenge_id: None,
            type_: FlagType::Static,
            content: "flag{x}".to_string(),
            data: None,
        }]);
        let mut local = base();
        local.flags = Some(vec![FlagContent::Detailed {
            id: None,
            challenge_id: None,
            type_: FlagType::Regex,
            content: "flag{x}".to_string(),
            data: None,
        }]);
        assert!(needs_update(&remote, &local));
    }

    // Fix 1: detailed flag with same content but different data triggers update
    #[test]
    fn flag_data_change_triggers_update() {
        let mut remote = base();
        remote.flags = Some(vec![FlagContent::Detailed {
            id: None,
            challenge_id: None,
            type_: FlagType::Static,
            content: "flag{x}".to_string(),
            data: Some(FlagData::CaseSensitive),
        }]);
        let mut local = base();
        local.flags = Some(vec![FlagContent::Detailed {
            id: None,
            challenge_id: None,
            type_: FlagType::Static,
            content: "flag{x}".to_string(),
            data: Some(FlagData::CaseInsensitive),
        }]);
        assert!(needs_update(&remote, &local));
    }

    // Fix 1: simple flag and detailed flag with same content are equivalent (no type/data diff)
    #[test]
    fn detailed_flag_same_content_as_simple_no_update() {
        let mut remote = base();
        remote.flags = Some(vec![FlagContent::Detailed {
            id: None,
            challenge_id: None,
            type_: FlagType::Static,
            content: "flag{x}".to_string(),
            data: None,
        }]);
        // local has same content as Simple; Simple maps to ("flag{x}", "static", "")
        // Detailed with Static + no data maps to ("flag{x}", "static", "") — no diff
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

    // Fix 2: hint cost change now IS detected
    #[test]
    fn hint_cost_change_triggers_update() {
        let mut remote = base();
        remote.hints = Some(vec![HintContent::Detailed {
            content: "look closer".to_string(),
            cost: Some(50),
            title: None,
        }]);
        let mut local = base();
        local.hints = Some(vec![HintContent::Detailed {
            content: "look closer".to_string(),
            cost: Some(100),
            title: None,
        }]);
        assert!(needs_update(&remote, &local));
    }

    // Fix 2: same hint content and cost — no update
    #[test]
    fn detailed_hint_same_content_and_cost_no_update() {
        let mut remote = base();
        remote.hints = Some(vec![HintContent::Detailed {
            content: "look closer".to_string(),
            cost: Some(50),
            title: None,
        }]);
        let mut local = base();
        local.hints = Some(vec![HintContent::Detailed {
            content: "look closer".to_string(),
            cost: Some(50),
            title: None,
        }]);
        assert!(!needs_update(&remote, &local));
    }

    // Fix 3: requirements content change triggers update
    #[test]
    fn requirements_content_change_triggers_update() {
        let mut remote = base();
        remote.requirements = Some(Requirements::Simple(vec![
            serde_json::Value::String("Warmup".to_string()),
        ]));
        let mut local = base();
        local.requirements = Some(Requirements::Simple(vec![
            serde_json::Value::String("Warmup".to_string()),
            serde_json::Value::String("Easy".to_string()),
        ]));
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn requirements_same_content_no_update() {
        let reqs = Some(Requirements::Simple(vec![
            serde_json::Value::String("Warmup".to_string()),
        ]));
        let mut remote = base();
        remote.requirements = reqs.clone();
        let mut local = base();
        local.requirements = reqs;
        assert!(!needs_update(&remote, &local));
    }

    #[test]
    fn requirements_none_vs_some_triggers_update() {
        let mut remote = base();
        remote.requirements = None;
        let mut local = base();
        local.requirements = Some(Requirements::Simple(vec![
            serde_json::Value::String("Warmup".to_string()),
        ]));
        assert!(needs_update(&remote, &local));
    }

    // Fix 4: instance config change triggers update
    #[test]
    fn instance_config_change_triggers_update() {
        let mut remote = base();
        remote.instance = Some(base_instance_config());
        let mut local = base();
        let mut cfg = base_instance_config();
        cfg.internal_port = 8080;
        local.instance = Some(cfg);
        assert!(needs_update(&remote, &local));
    }

    #[test]
    fn instance_config_same_no_update() {
        let mut remote = base();
        remote.instance = Some(base_instance_config());
        let mut local = base();
        local.instance = Some(base_instance_config());
        assert!(!needs_update(&remote, &local));
    }

    #[test]
    fn instance_config_none_vs_some_triggers_update() {
        let mut remote = base();
        remote.instance = None;
        let mut local = base();
        local.instance = Some(base_instance_config());
        assert!(needs_update(&remote, &local));
    }
}
