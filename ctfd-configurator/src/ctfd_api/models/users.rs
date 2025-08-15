use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: Option<u32>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub website: Option<String>,
    pub affiliation: Option<String>,
    pub country: Option<String>,
    pub bracket: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub verified: Option<bool>,
    pub hidden: Option<bool>,
    pub banned: Option<bool>,
    pub team_id: Option<u32>,
    pub score: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct UserCreate {
    pub name: Option<String>,
    pub password: Option<String>,
    pub email: Option<String>,
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
    pub field: Option<String>,
    pub value: Option<String>,
}
