use crate::ctfd_api::models::teams::Team;
use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::json;
use serde_json::Value;

impl CtfdClient {
    /// GET /teams - List all teams
    pub async fn get_teams(&self) -> Result<Vec<Team>> {
        self.execute(Method::GET, "/teams", None::<&()>).await
    }

    /// GET /teams/{team_id} - Get a specific team
    pub async fn get_team(&self, team_id: u32) -> Result<Team> {
        self.execute(Method::GET, &format!("/teams/{}", team_id), None::<&()>)
            .await
    }

    /// POST /teams - Create a new team
    pub async fn create_team(&self, team_data: &Value) -> Result<Team> {
        self.execute(Method::POST, "/teams", Some(team_data)).await
    }

    /// PATCH /teams/{team_id} - Update a team
    pub async fn update_team(&self, team_id: u32, update_data: &Value) -> Result<Team> {
        self.execute(
            Method::PATCH,
            &format!("/teams/{}", team_id),
            Some(update_data),
        )
        .await
    }

    /// DELETE /teams/{team_id} - Delete a team
    pub async fn delete_team(&self, team_id: u32) -> Result<()> {
        self.request(Method::DELETE, &format!("/teams/{}", team_id), None::<&()>)
            .await?;
        Ok(())
    }

    /// GET /teams/me - Get the current user's team
    pub async fn get_my_team(&self) -> Result<Team> {
        self.execute(Method::GET, "/teams/me", None::<&()>).await
    }

    /// GET /teams?name={name} - Search teams by name
    pub async fn search_teams_by_name(&self, name: &str) -> Result<Vec<Team>> {
        let params = [("name", name)];
        self.execute_with_params(Method::GET, "/teams", None::<&()>, &params)
            .await
    }

    /// GET /teams/{team_id}/members - Get team members
    pub async fn get_team_members(&self, team_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/teams/{}/members", team_id),
            None::<&()>,
        )
        .await
    }

    /// POST /teams/{team_id}/members - Add member to team
    pub async fn add_team_member(&self, team_id: u32, user_id: u32) -> Result<()> {
        let body = json!({ "user_id": user_id });
        self.request(
            Method::POST,
            &format!("/teams/{}/members", team_id),
            Some(&body),
        )
        .await?;
        Ok(())
    }

    /// DELETE /teams/{team_id}/members/{user_id} - Remove member from team
    pub async fn remove_team_member(&self, team_id: u32, user_id: u32) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/teams/{}/members/{}", team_id, user_id),
            None::<&()>,
        )
        .await?;
        Ok(())
    }
}
