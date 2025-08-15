use crate::ctfd_api::CtfdClient;
use crate::ctfd_api::models::scoreboard::ScoreboardEntry;
use anyhow::Result;
use reqwest::Method;

impl CtfdClient {
    /// GET /scoreboard - Get the full scoreboard
    pub async fn get_scoreboard(&self) -> Result<Vec<ScoreboardEntry>> {
        self.execute(Method::GET, "/scoreboard", None::<&()>).await
    }

    /// GET /scoreboard/top/{count} - Get top teams
    pub async fn get_top_teams(&self, count: u32) -> Result<Vec<ScoreboardEntry>> {
        self.execute(
            Method::GET,
            &format!("/scoreboard/top/{}", count),
            None::<&()>,
        )
        .await
    }

    /// GET /scoreboard/details - Get detailed scoreboard information
    pub async fn get_detailed_scoreboard(&self) -> Result<serde_json::Value> {
        self.execute(Method::GET, "/scoreboard/details", None::<&()>)
            .await
    }

    /// GET /scoreboard/teams - Get team-based scoreboard
    pub async fn get_team_scoreboard(&self) -> Result<Vec<ScoreboardEntry>> {
        self.execute(Method::GET, "/scoreboard/teams", None::<&()>)
            .await
    }

    /// GET /scoreboard/users - Get user-based scoreboard
    pub async fn get_user_scoreboard(&self) -> Result<Vec<ScoreboardEntry>> {
        self.execute(Method::GET, "/scoreboard/users", None::<&()>)
            .await
    }
}
