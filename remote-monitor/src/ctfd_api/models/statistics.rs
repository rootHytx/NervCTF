use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Statistics {
    pub solves: SolveStatistics,
    pub score_distribution: ScoreDistribution,
    pub challenge_stats: ChallengeStatistics,
    pub user_stats: UserStatistics,
    pub team_stats: TeamStatistics,
}

#[derive(Debug, Deserialize)]
pub struct SolveStatistics {
    pub solves: u32,
    pub fails: u32,
    pub total: u32,
    pub solve_percent: f32,
    pub fail_percent: f32,
}

#[derive(Debug, Deserialize)]
pub struct ScoreDistribution {
    pub brackets: Vec<ScoreBracket>,
    pub average: f32,
    pub median: f32,
    pub top: Vec<ScoreEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ScoreBracket {
    pub score: u32,
    pub count: u32,
}

#[derive(Debug, Deserialize)]
pub struct ScoreEntry {
    pub account_id: u32,
    pub account_name: String,
    pub score: u32,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeStatistics {
    pub total: u32,
    pub solved: u32,
    pub unsolved: u32,
    pub per_category: Vec<CategoryStats>,
    pub per_value: Vec<ValueStats>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryStats {
    pub category: String,
    pub count: u32,
}

#[derive(Debug, Deserialize)]
pub struct ValueStats {
    pub value: u32,
    pub count: u32,
}

#[derive(Debug, Deserialize)]
pub struct UserStatistics {
    pub total: u32,
    pub confirmed: u32,
    pub unconfirmed: u32,
    pub active: u32,
    pub inactive: u32,
    pub banned: u32,
    pub per_country: Vec<CountryStats>,
}

#[derive(Debug, Deserialize)]
pub struct TeamStatistics {
    pub total: u32,
    pub active: u32,
    pub inactive: u32,
    pub banned: u32,
    pub per_country: Vec<CountryStats>,
    pub sizes: Vec<TeamSizeStats>,
}

#[derive(Debug, Deserialize)]
pub struct CountryStats {
    pub country: String,
    pub count: u32,
}

#[derive(Debug, Deserialize)]
pub struct TeamSizeStats {
    pub size: u32,
    pub count: u32,
}
