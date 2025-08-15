use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Token {
    pub id: u32,
    pub user_id: u32,
    pub expiration: String,
    pub value: String,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenCreate {
    pub expiration: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenUpdate {
    pub expiration: Option<String>,
    pub description: Option<String>,
}
