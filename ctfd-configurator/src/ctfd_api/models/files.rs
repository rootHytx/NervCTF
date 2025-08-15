use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct File {
    pub id: Option<u32>,
    pub challenge_id: Option<u32>,
    pub location: Option<String>,
    pub filename: Option<String>,
    pub size: Option<u64>,
    pub mimetype: Option<String>,
    pub description: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FileCreate {
    pub challenge_id: Option<u32>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
    pub filename: Option<String>,
    pub description: Option<String>,
}
