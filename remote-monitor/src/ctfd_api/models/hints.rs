use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Hint {
    pub id: u32,
    pub challenge_id: Option<u32>,
    pub content: String,
    pub cost: u32,
    pub requirements: Option<serde_json::Value>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HintCreate {
    pub challenge_id: u32,
    pub content: String,
    pub cost: u32,
    pub requirements: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct HintUpdate {
    pub content: Option<String>,
    pub cost: Option<u32>,
    pub requirements: Option<serde_json::Value>,
}
