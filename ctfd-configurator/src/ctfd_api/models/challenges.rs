use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Challenge {
    pub id: Option<u32>,
    pub name: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub value: Option<u32>,
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
    pub name: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub value: Option<u32>,
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
    pub id: Option<u32>,
    pub name: Option<String>,
    pub templates: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeAttempt {
    pub id: Option<u32>,
    pub challenge_id: Option<u32>,
    pub user_id: Option<u32>,
    pub team_id: Option<u32>,
    pub ip: Option<String>,
    pub provided: Option<String>,
    pub date: Option<String>,
    #[serde(rename = "type")]
    pub type_field: Option<String>,
}
