use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Token {
    pub id: Option<u32>,
    pub user_id: Option<u32>,
    pub expiration: Option<String>,
    pub value: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenCreate {
    pub expiration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenUpdate {
    pub expiration: Option<String>,
    pub description: Option<String>,
}
