use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ScoreboardEntry {
    pub pos: Option<u32>,
    pub account_id: Option<u32>,
    pub account_url: Option<String>,
    pub account_name: Option<String>,
    pub score: Option<u32>,
    pub solves: Option<u32>,
    pub member_count: Option<u32>,
    pub team_id: Option<u32>,
    pub team_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ScoreboardGraph {
    pub labels: Option<Vec<String>>,
    pub datasets: Option<Vec<ScoreboardDataset>>,
}

#[derive(Debug, Deserialize)]
pub struct ScoreboardDataset {
    pub label: Option<String>,
    pub data: Option<Vec<u32>>,
    pub border_color: Option<String>,
    pub background_color: Option<String>,
    pub fill: Option<bool>,
}
