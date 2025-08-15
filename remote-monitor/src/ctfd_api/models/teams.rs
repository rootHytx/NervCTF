use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Team {
    pub id: u32,
    pub name: String,
    pub email: Option<String>,
    pub website: Option<String>,
    pub affiliation: Option<String>,
    pub country: Option<String>,
    pub bracket: Option<String>,
    pub members: Vec<TeamMember>,
    pub captain_id: Option<u32>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TeamMember {
    pub user_id: u32,
    pub user_name: String,
    pub score: u32,
}

#[derive(Debug, Serialize)]
pub struct TeamCreate {
    pub name: String,
    pub email: Option<String>,
    pub password: String,
    pub website: Option<String>,
    pub affiliation: Option<String>,
    pub country: Option<String>,
    pub bracket: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TeamUpdate {
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
    pub website: Option<String>,
    pub affiliation: Option<String>,
    pub country: Option<String>,
    pub bracket: Option<String>,
    pub captain_id: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct TeamInvite {
    pub user_id: u32,
}

#[derive(Debug, Deserialize)]
pub struct TeamInviteResponse {
    pub invite_code: String,
}
