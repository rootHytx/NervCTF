use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ScoreboardEntry {
    pub pos: u32,
    pub account_id: u32,
    pub account_url: String,
    pub account_name: String,
    pub score: u32,
    pub solves: Option<u32>,
    pub member_count: Option<u32>,
    pub team_id: Option<u32>,
    pub team_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ScoreboardGraph {
    pub labels: Vec<String>,
    pub datasets: Vec<ScoreboardDataset>,
}

#[derive(Debug, Deserialize)]
pub struct ScoreboardDataset {
    pub label: String,
    pub data: Vec<u32>,
    pub border_color: Option<String>,
    pub background_color: Option<String>,
    pub fill: Option<bool>,
}
