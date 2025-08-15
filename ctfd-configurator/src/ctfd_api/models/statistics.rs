use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Statistics {
    pub solves: Option<SolveStatistics>,
    pub score_distribution: Option<ScoreDistribution>,
    pub challenge_stats: Option<ChallengeStatistics>,
    pub user_stats: Option<UserStatistics>,
    pub team_stats: Option<TeamStatistics>,
}

#[derive(Debug, Deserialize)]
pub struct SolveStatistics {
    pub solves: Option<u32>,
    pub fails: Option<u32>,
    pub total: Option<u32>,
    pub solve_percent: Option<f32>,
    pub fail_percent: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ScoreDistribution {
    pub brackets: Option<Vec<ScoreBracket>>,
    pub average: Option<f32>,
    pub median: Option<f32>,
    pub top: Option<Vec<ScoreEntry>>,
}

#[derive(Debug, Deserialize)]
pub struct ScoreBracket {
    pub score: Option<u32>,
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct ScoreEntry {
    pub account_id: Option<u32>,
    pub account_name: Option<String>,
    pub score: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeStatistics {
    pub total: Option<u32>,
    pub solved: Option<u32>,
    pub unsolved: Option<u32>,
    pub per_category: Option<Vec<CategoryStats>>,
    pub per_value: Option<Vec<ValueStats>>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryStats {
    pub category: Option<String>,
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct ValueStats {
    pub value: Option<u32>,
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct UserStatistics {
    pub total: Option<u32>,
    pub confirmed: Option<u32>,
    pub unconfirmed: Option<u32>,
    pub active: Option<u32>,
    pub inactive: Option<u32>,
    pub banned: Option<u32>,
    pub per_country: Option<Vec<CountryStats>>,
}

#[derive(Debug, Deserialize)]
pub struct TeamStatistics {
    pub total: Option<u32>,
    pub active: Option<u32>,
    pub inactive: Option<u32>,
    pub banned: Option<u32>,
    pub per_country: Option<Vec<CountryStats>>,
    pub sizes: Option<Vec<TeamSizeStats>>,
}

#[derive(Debug, Deserialize)]
pub struct CountryStats {
    pub country: Option<String>,
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct TeamSizeStats {
    pub size: Option<u32>,
    pub count: Option<u32>,
}
