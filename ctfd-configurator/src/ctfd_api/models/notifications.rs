use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Notification {
    pub id: Option<u32>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub user_id: Option<u32>,
    pub team_id: Option<u32>,
    pub date: Option<String>,
    pub sound: Option<bool>,
    pub html: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NotificationCreate {
    pub title: Option<String>,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct NotificationUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<bool>,
}
