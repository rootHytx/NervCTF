use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Statistics {
    pub solves: serde_json::Value,
    pub score_distribution: serde_json::Value,
    pub challenge_solve_counts: Option<serde_json::Value>,
    pub solve_percentages: Option<serde_json::Value>,
    pub team_statistics: Option<serde_json::Value>,
    pub user_statistics: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeSolveCount {
    pub challenge_id: u32,
    pub solve_count: u32,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeSolvePercentage {
    pub challenge_id: u32,
    pub percentage: f32,
}

impl CtfdClient {
    /// GET /statistics - Get overall statistics
    pub async fn get_statistics(&self) -> Result<Statistics> {
        self.execute(Method::GET, "/statistics", None::<&()>).await
    }

    /// GET /statistics/challenges/solves - Get solve counts per challenge
    pub async fn get_challenge_solve_counts(&self) -> Result<Vec<ChallengeSolveCount>> {
        self.execute(Method::GET, "/statistics/challenges/solves", None::<&()>)
            .await
    }

    /// GET /statistics/challenges/solves/percent - Get solve percentages per challenge
    pub async fn get_challenge_solve_percentages(&self) -> Result<Vec<ChallengeSolvePercentage>> {
        self.execute(
            Method::GET,
            "/statistics/challenges/solves/percent",
            None::<&()>,
        )
        .await
    }

    /// GET /statistics/teams - Get team statistics
    pub async fn get_team_statistics(&self) -> Result<serde_json::Value> {
        self.execute(Method::GET, "/statistics/teams", None::<&()>)
            .await
    }

    /// GET /statistics/users - Get user statistics
    pub async fn get_user_statistics(&self) -> Result<serde_json::Value> {
        self.execute(Method::GET, "/statistics/users", None::<&()>)
            .await
    }
}
