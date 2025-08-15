use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct File {
    pub id: u32,
    pub challenge_id: Option<u32>,
    pub location: String,
    pub filename: String,
    pub size: u64,
    pub mimetype: Option<String>,
    pub description: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FileCreate {
    pub challenge_id: Option<u32>,
    #[serde(rename = "type")]
    pub file_type: String,
    pub filename: String,
    pub description: Option<String>,
}
