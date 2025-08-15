use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Flag {
    pub id: Option<u32>,
    pub challenge_id: Option<u32>,
    pub content: Option<String>,
    pub data: Option<String>,
    #[serde(rename = "type")]
    pub flag_type: Option<String>,
    pub description: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FlagCreate {
    pub challenge_id: Option<u32>,
    pub content: Option<String>,
    pub data: Option<String>,
    #[serde(rename = "type")]
    pub flag_type: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FlagUpdate {
    pub content: Option<String>,
    pub data: Option<String>,
    #[serde(rename = "type")]
    pub flag_type: Option<String>,
    pub description: Option<String>,
}
