use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Challenge {
    pub id: u32,
    pub name: String,
    pub category: String,
    pub description: String,
    pub value: u32,
    pub connection_info: Option<String>,
    pub next_id: Option<u32>,
    pub max_attempts: Option<u32>,
    pub state: Option<String>,
    pub requirements: Option<serde_json::Value>,
    #[serde(rename = "type")]
    pub type_field: Option<String>,
    pub type_data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct ChallengeCreate {
    pub name: String,
    pub category: String,
    pub description: String,
    pub value: u32,
    pub connection_info: Option<String>,
    pub max_attempts: Option<u32>,
    pub state: Option<String>,
    pub requirements: Option<serde_json::Value>,
    #[serde(rename = "type")]
    pub type_field: Option<String>,
    pub type_data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeType {
    pub id: u32,
    pub name: String,
    pub templates: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeAttempt {
    pub id: u32,
    pub challenge_id: u32,
    pub user_id: u32,
    pub team_id: u32,
    pub ip: String,
    pub provided: String,
    pub date: String,
    #[serde(rename = "type")]
    pub type_field: String,
}
