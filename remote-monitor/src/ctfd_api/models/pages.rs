use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Page {
    pub id: u32,
    pub title: String,
    pub route: String,
    pub content: String,
    pub draft: bool,
    pub hidden: bool,
    pub auth_required: bool,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PageCreate {
    pub title: String,
    pub route: String,
    pub content: String,
    pub draft: Option<bool>,
    pub hidden: Option<bool>,
    pub auth_required: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct PageUpdate {
    pub title: Option<String>,
    pub route: Option<String>,
    pub content: Option<String>,
    pub draft: Option<bool>,
    pub hidden: Option<bool>,
    pub auth_required: Option<bool>,
}
