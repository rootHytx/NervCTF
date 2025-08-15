use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: u32,
    pub name: String,
    pub email: Option<String>,
    pub website: Option<String>,
    pub affiliation: Option<String>,
    pub country: Option<String>,
    pub bracket: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub verified: bool,
    pub hidden: bool,
    pub banned: bool,
    pub team_id: Option<u32>,
    pub score: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct UserCreate {
    pub name: String,
    pub password: String,
    pub email: String,
    pub website: Option<String>,
    pub affiliation: Option<String>,
    pub country: Option<String>,
    pub bracket: Option<String>,
    pub verified: Option<bool>,
    pub hidden: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct UserUpdate {
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
    pub website: Option<String>,
    pub affiliation: Option<String>,
    pub country: Option<String>,
    pub bracket: Option<String>,
    pub verified: Option<bool>,
    pub hidden: Option<bool>,
    pub banned: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserSearch {
    pub field: String,
    pub value: String,
}
