use crate::challenge_manager::sync::SyncAction;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Deserialize, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChallengeType {
    Standard,
    Dynamic,
    Container,
}

#[derive(Debug, Deserialize, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FlagType {
    Static,
    Regex,
}

/// Bug 2 fix: use snake_case so CaseInsensitive → "case_insensitive"
#[derive(Debug, Deserialize, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FlagData {
    CaseSensitive,
    CaseInsensitive,
}

/// Gap 2 fix: `data` is now optional in Detailed variant
#[derive(Debug, Deserialize, Clone, Serialize)]
#[serde(untagged)]
pub enum FlagContent {
    Simple(String),
    Detailed {
        id: Option<u32>,
        challenge_id: Option<u32>,
        #[serde(rename = "type")]
        type_: FlagType,
        content: String,
        data: Option<FlagData>,
    },
}

/// Kept for CTFd API responses (always returns detailed format)
#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct Hint {
    pub content: String,
    pub cost: Option<u32>,
    pub title: Option<String>,
}

/// Gap 1 fix: HintContent supports both simple string and detailed object
#[derive(Debug, Deserialize, Clone, Serialize)]
#[serde(untagged)]
pub enum HintContent {
    Simple(String),
    Detailed {
        content: String,
        cost: Option<u32>,
        title: Option<String>,
    },
}

impl HintContent {
    pub fn content_str(&self) -> &str {
        match self {
            HintContent::Simple(s) => s.as_str(),
            HintContent::Detailed { content, .. } => content.as_str(),
        }
    }
}

/// Flag generation mode for container challenges.
#[derive(Debug, Deserialize, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FlagMode {
    Static,
    Random,
}

/// `extra` block — shared by Dynamic and Container challenge types.
/// Dynamic uses only the scoring fields; Container uses all of them.
/// All fields are optional so serde silently ignores unknown sub-keys.
#[derive(Debug, Deserialize, Clone, Serialize, Default)]
pub struct Extra {
    // ── Scoring (dynamic + container) ────────────────────────────────────────
    pub initial: Option<u32>,
    pub decay: Option<u32>,
    pub minimum: Option<u32>,

    // ── Container-specific ────────────────────────────────────────────────────
    /// Docker image name or local build path (`.` = build from Dockerfile).
    pub image: Option<String>,
    /// TCP port the container service listens on (integer form).
    pub internal_port: Option<u32>,
    /// TCP port as a string (some plugins prefer this form).
    pub internal_ports: Option<String>,
    /// How the flag is generated: `static` (from `flags:`) or `random`.
    pub flag_mode: Option<FlagMode>,
    /// Prefix prepended to random flags (e.g. `"upCTF{"`).
    pub flag_prefix: Option<String>,
    /// Suffix appended to random flags (e.g. `"}"`).
    pub flag_suffix: Option<String>,
    /// Length of the random part of an auto-generated flag.
    pub random_flag_length: Option<u32>,
    /// Scoring decay function: `"linear"` or `"logarithmic"`.
    pub decay_function: Option<String>,
    /// How many minutes before an idle container is destroyed.
    pub timeout_minutes: Option<u32>,
    /// Override entrypoint / command run inside the container.
    pub command: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Hidden,
    Visible,
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct File {
    pub location: String,
    pub sha1sum: String,
    pub id: Option<u32>,
    #[serde(rename = "type")]
    pub file_type: String,
}

#[derive(Debug, Deserialize, Clone, Serialize)]
#[serde(untagged)]
pub enum Tag {
    Simple(String),
    Detailed {
        challenge_id: Option<u32>,
        id: Option<u32>,
        value: String,
    },
}

impl Tag {
    pub fn value_str(&self) -> &str {
        match self {
            Tag::Simple(s) => s.as_str(),
            Tag::Detailed { value, .. } => value.as_str(),
        }
    }
}

/// Gap 3 fix: Requirements supports simple list, advanced object, and integer IDs
#[derive(Debug, Deserialize, Clone, Serialize)]
#[serde(untagged)]
pub enum Requirements {
    Simple(Vec<serde_json::Value>),
    Advanced {
        prerequisites: Vec<serde_json::Value>,
        #[serde(default)]
        anonymize: bool,
    },
}

impl Requirements {
    /// Extract prerequisite names/IDs as strings for topological sorting
    pub fn prerequisite_names(&self) -> Vec<String> {
        let vals = match self {
            Requirements::Simple(v) => v,
            Requirements::Advanced { prerequisites, .. } => prerequisites,
        };
        vals.iter()
            .filter_map(|v| {
                if let Some(s) = v.as_str() {
                    Some(s.to_string())
                } else if let Some(n) = v.as_u64() {
                    Some(n.to_string())
                } else {
                    None
                }
            })
            .collect()
    }
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct Challenge {
    pub name: String,
    pub category: String,
    pub value: u32,
    #[serde(rename = "type")]
    pub challenge_type: ChallengeType,

    // Optional fields
    pub description: Option<String>,
    pub id: Option<u32>,
    pub challenge_id: Option<u32>,
    pub author: Option<String>,
    pub extra: Option<Extra>,
    pub image: Option<String>,
    pub protocol: Option<String>,
    pub host: Option<String>,
    pub connection_info: Option<String>,
    pub healthcheck: Option<String>,
    pub attempts: Option<u32>,
    pub flags: Option<Vec<FlagContent>>,
    pub topics: Option<Vec<String>>,
    pub tags: Option<Vec<Tag>>,
    pub files: Option<Vec<String>>,
    /// Gap 1 fix: hints support both simple strings and detailed objects
    pub hints: Option<Vec<HintContent>>,
    /// Gap 3 fix: requirements support simple list and advanced object
    pub requirements: Option<Requirements>,
    pub next: Option<String>,
    pub state: Option<State>,
    #[serde(skip)]
    pub script: Option<String>,
    #[serde(skip)]
    pub solved_by_me: Option<bool>,
    #[serde(skip)]
    pub solves: Option<u32>,
    #[serde(skip)]
    pub template: Option<String>,

    #[serde(default = "default_version")]
    pub version: String,
    #[serde(skip)]
    pub source_path: String,
    /// Top-level YAML keys from the source file not recognised by the
    /// ctfcli spec. Populated by DirectoryScanner; never serialised or
    /// sent to the CTFd API.
    #[serde(skip)]
    pub unknown_yaml_keys: Vec<String>,
}

fn default_version() -> String {
    "0.1".to_string()
}

impl Challenge {
    /// Serialize the Challenge struct to a YAML string.
    pub fn to_yaml_string(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct ChallengeWaiting {
    name: String,
    requirements: Vec<String>,
}
impl ChallengeWaiting {
    pub fn new(name: String, requirements: Vec<String>) -> Self {
        Self { name, requirements }
    }

    pub fn satisfied(&self) -> bool {
        self.requirements.is_empty()
    }
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct RequirementsQueue {
    queue: Vec<ChallengeWaiting>,
}

impl RequirementsQueue {
    pub fn new() -> Self {
        Self { queue: Vec::new() }
    }

    pub fn add(&mut self, challenge: String, requirements: Vec<String>) {
        self.queue
            .push(ChallengeWaiting::new(challenge, requirements));
    }

    pub fn process(&mut self, solved_challenge: &str) {
        self.queue.iter_mut().for_each(|cw| {
            cw.requirements.retain(|req| req != solved_challenge);
        });
        self.queue.retain(|cw| !cw.satisfied());
    }

    pub fn pop_satisfied(&mut self) -> Vec<String> {
        let mut satisfied = Vec::new();
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].satisfied() {
                satisfied.push(self.queue.remove(i).name.clone());
            } else {
                i += 1;
            }
        }
        satisfied
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn contains(&self, challenge_name: &str) -> bool {
        self.queue.iter().any(|cw| cw.name == challenge_name)
    }

    pub fn refresh(&mut self, challenge: String) {
        self.queue.iter_mut().for_each(|cw| {
            cw.requirements.retain(|req| req != &challenge);
        });
        self.queue.retain(|cw| !cw.satisfied());
    }

    pub fn print(&self) {
        if self.queue.is_empty() {
            println!("✅ All challenges are ready to be processed.");
        } else {
            println!("⏳ Challenges waiting for dependencies:");
            for cw in &self.queue {
                println!(
                    "  - {} (waiting for: {})",
                    cw.name,
                    cw.requirements.join(", ")
                );
            }
        }
    }

    pub fn resolve_dependencies<'a>(&self, actions: Vec<SyncAction<'a>>) -> Vec<SyncAction<'a>> {
        // Separate actions
        let mut to_sort = Vec::new();
        let mut to_append = Vec::new();

        for action in actions {
            match action {
                SyncAction::Create { .. } | SyncAction::Update { .. } => to_sort.push(action),
                SyncAction::UpToDate { .. } | SyncAction::RemoteOnly { .. } => {
                    to_append.push(action)
                }
            }
        }

        // Build dependency map with owned Strings (Requirements enum returns Vec<String>)
        let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
        let mut name_to_action: HashMap<&str, &SyncAction> = HashMap::new();

        for action in &to_sort {
            let (name, requirements): (&str, HashSet<String>) = match action {
                SyncAction::Create { name, challenge } => (
                    name.as_str(),
                    challenge
                        .requirements
                        .as_ref()
                        .map(|r| r.prerequisite_names().into_iter().collect())
                        .unwrap_or_default(),
                ),
                SyncAction::Update { name, local, .. } => (
                    name.as_str(),
                    local
                        .requirements
                        .as_ref()
                        .map(|r| r.prerequisite_names().into_iter().collect())
                        .unwrap_or_default(),
                ),
                _ => continue,
            };
            deps.insert(name.to_string(), requirements);
            name_to_action.insert(name, action);
        }

        // Kahn's algorithm for topological sort
        let mut sorted = Vec::new();
        let mut no_deps: VecDeque<String> = deps
            .iter()
            .filter(|(_, reqs)| reqs.is_empty())
            .map(|(name, _)| name.clone())
            .collect();

        let mut processed: HashSet<String> = HashSet::new();

        while let Some(name) = no_deps.pop_front() {
            if processed.contains(&name) {
                continue;
            }
            processed.insert(name.clone());

            if let Some(action) = name_to_action.get(name.as_str()) {
                sorted.push((*action).clone());
            }
            // Remove this name from other requirements
            for reqs in deps.values_mut() {
                reqs.remove(&name);
            }
            // Find new nodes with no dependencies and not yet processed
            for (n, reqs) in deps.iter() {
                if reqs.is_empty() && !processed.contains(n) {
                    no_deps.push_back(n.clone());
                }
            }
        }

        // Append the unsorted actions at the end
        sorted.extend(to_append);
        sorted
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── YAML deserialization ──────────────────────────────────────────────────

    #[test]
    fn parse_minimal_yaml() {
        let yaml = r#"
name: "Simple"
category: "web"
value: 100
type: standard
flags:
  - flag{test}
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.name, "Simple");
        assert_eq!(c.category, "web");
        assert_eq!(c.value, 100);
        assert_eq!(c.challenge_type, ChallengeType::Standard);
        assert!(c.flags.is_some());
    }

    #[test]
    fn parse_dynamic_challenge() {
        let yaml = r#"
name: "Dynamic"
category: "pwn"
value: 0
type: dynamic
flags:
  - flag{dynamic}
extra:
  initial: 500
  decay: 50
  minimum: 100
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.challenge_type, ChallengeType::Dynamic);
        let extra = c.extra.unwrap();
        assert_eq!(extra.initial, Some(500));
        assert_eq!(extra.decay, Some(50));
        assert_eq!(extra.minimum, Some(100));
    }

    #[test]
    fn parse_state_visible() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
state: visible"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.state, Some(State::Visible));
    }

    #[test]
    fn parse_state_hidden() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
state: hidden"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.state, Some(State::Hidden));
    }

    // ── Flags ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_simple_flag_string() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags:
  - flag{simple}
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let flags = c.flags.unwrap();
        assert_eq!(flags.len(), 1);
        match &flags[0] {
            FlagContent::Simple(s) => assert_eq!(s, "flag{simple}"),
            _ => panic!("Expected Simple flag"),
        }
    }

    #[test]
    fn parse_detailed_flag_with_data() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags:
  - type: static
    content: "flag{detailed}"
    data: case_insensitive
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let flags = c.flags.unwrap();
        match &flags[0] {
            FlagContent::Detailed { content, data, type_, .. } => {
                assert_eq!(content, "flag{detailed}");
                assert_eq!(*type_, FlagType::Static);
                assert_eq!(*data, Some(FlagData::CaseInsensitive));
            }
            _ => panic!("Expected Detailed flag"),
        }
    }

    #[test]
    fn parse_detailed_flag_without_data() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags:
  - type: static
    content: "flag{no_data}"
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let flags = c.flags.unwrap();
        match &flags[0] {
            FlagContent::Detailed { content, data, .. } => {
                assert_eq!(content, "flag{no_data}");
                assert!(data.is_none());
            }
            _ => panic!("Expected Detailed flag"),
        }
    }

    #[test]
    fn parse_regex_flag_type() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags:
  - type: regex
    content: "flag\\{.*\\}"
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let flags = c.flags.unwrap();
        match &flags[0] {
            FlagContent::Detailed { type_, .. } => assert_eq!(*type_, FlagType::Regex),
            _ => panic!("Expected Detailed flag"),
        }
    }

    // ── FlagData serialization ────────────────────────────────────────────────

    #[test]
    fn flag_data_case_insensitive_serializes_correctly() {
        // Must serialize as "case_insensitive" (snake_case), not "caseinsensitive"
        let val = serde_json::to_value(FlagData::CaseInsensitive).unwrap();
        assert_eq!(val, serde_json::Value::String("case_insensitive".to_string()));
    }

    #[test]
    fn flag_data_case_sensitive_serializes_correctly() {
        let val = serde_json::to_value(FlagData::CaseSensitive).unwrap();
        assert_eq!(val, serde_json::Value::String("case_sensitive".to_string()));
    }

    #[test]
    fn flag_data_case_insensitive_roundtrip() {
        let original = FlagData::CaseInsensitive;
        let json = serde_json::to_string(&original).unwrap();
        let parsed: FlagData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    // ── Hints ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_simple_hint() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
hints:
  - "check the source"
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let hints = c.hints.unwrap();
        match &hints[0] {
            HintContent::Simple(s) => assert_eq!(s, "check the source"),
            _ => panic!("Expected Simple hint"),
        }
    }

    #[test]
    fn parse_detailed_hint_with_cost() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
hints:
  - content: "look at headers"
    cost: 50
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let hints = c.hints.unwrap();
        match &hints[0] {
            HintContent::Detailed { content, cost, .. } => {
                assert_eq!(content, "look at headers");
                assert_eq!(*cost, Some(50));
            }
            _ => panic!("Expected Detailed hint"),
        }
    }

    #[test]
    fn parse_mixed_hints() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
hints:
  - "free hint"
  - content: "paid hint"
    cost: 100
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let hints = c.hints.unwrap();
        assert_eq!(hints.len(), 2);
        assert!(matches!(&hints[0], HintContent::Simple(_)));
        assert!(matches!(&hints[1], HintContent::Detailed { .. }));
    }

    // ── Tags ──────────────────────────────────────────────────────────────────

    #[test]
    fn parse_simple_tag() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
tags:
  - web
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let tags = c.tags.unwrap();
        match &tags[0] {
            Tag::Simple(s) => assert_eq!(s, "web"),
            _ => panic!("Expected Simple tag"),
        }
    }

    // ── Requirements ──────────────────────────────────────────────────────────

    #[test]
    fn parse_requirements_simple_name_list() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
requirements:
  - "Warmup"
  - "Easy"
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let names = c.requirements.unwrap().prerequisite_names();
        assert_eq!(names, vec!["Warmup", "Easy"]);
    }

    #[test]
    fn parse_requirements_integer_ids() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
requirements:
  - 1
  - 3
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        let names = c.requirements.unwrap().prerequisite_names();
        assert_eq!(names, vec!["1", "3"]);
    }

    #[test]
    fn parse_requirements_advanced_object() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
requirements:
  prerequisites:
    - "Warmup"
  anonymize: true
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        match c.requirements.unwrap() {
            Requirements::Advanced { prerequisites, anonymize } => {
                assert_eq!(prerequisites.len(), 1);
                assert!(anonymize);
            }
            _ => panic!("Expected Advanced requirements"),
        }
    }

    #[test]
    fn prerequisite_names_filters_nulls() {
        let reqs = Requirements::Simple(vec![
            serde_json::Value::String("Warmup".to_string()),
            serde_json::Value::Null,
            serde_json::Value::Number(5.into()),
        ]);
        let names = reqs.prerequisite_names();
        assert_eq!(names, vec!["Warmup", "5"]);
    }

    // ── HintContent helper ────────────────────────────────────────────────────

    #[test]
    fn hint_content_str_simple() {
        let h = HintContent::Simple("check cookies".to_string());
        assert_eq!(h.content_str(), "check cookies");
    }

    #[test]
    fn hint_content_str_detailed() {
        let h = HintContent::Detailed {
            content: "look harder".to_string(),
            cost: Some(50),
            title: None,
        };
        assert_eq!(h.content_str(), "look harder");
    }

    // ── Tag helper ────────────────────────────────────────────────────────────

    #[test]
    fn tag_value_str_simple() {
        let t = Tag::Simple("sql".to_string());
        assert_eq!(t.value_str(), "sql");
    }

    #[test]
    fn tag_value_str_detailed() {
        let t = Tag::Detailed {
            challenge_id: None,
            id: None,
            value: "xss".to_string(),
        };
        assert_eq!(t.value_str(), "xss");
    }

    // ── version default ───────────────────────────────────────────────────────

    #[test]
    fn version_defaults_to_01_when_omitted() {
        let yaml = r#"name: x
category: y
value: 1
type: standard
flags: ["flag{x}"]
"#;
        let c: Challenge = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.version, "0.1");
    }
}
