use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Tag {
    pub id: Option<u32>,
    pub challenge_id: Option<u32>,
    pub value: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TagCreate {
    pub challenge_id: Option<u32>,
    pub value: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TagUpdate {
    pub value: Option<String>,
    pub challenge_id: Option<u32>,
}
