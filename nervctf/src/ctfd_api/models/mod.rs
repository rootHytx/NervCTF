use crate::challenge_manager::sync::SyncAction;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Deserialize, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChallengeType {
    Standard,
    Dynamic,
    // Add other types as needed
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

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct Extra {
    pub initial: Option<u32>,
    pub decay: Option<u32>,
    pub minimum: Option<u32>,
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
