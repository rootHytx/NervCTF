use crate::ctfd_api::CtfdClient;
use crate::ctfd_api::models::users::User;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// GET /users - List all users
    pub async fn get_users(&self) -> Result<Vec<User>> {
        self.execute(Method::GET, "/users", None::<&()>).await
    }

    /// GET /users/{user_id} - Get a specific user
    pub async fn get_user(&self, user_id: u32) -> Result<User> {
        self.execute(Method::GET, &format!("/users/{}", user_id), None::<&()>)
            .await
    }

    /// POST /users - Create a new user
    pub async fn create_user(&self, user_data: &Value) -> Result<User> {
        self.execute(Method::POST, "/users", Some(user_data)).await
    }

    /// PATCH /users/{user_id} - Update a user
    pub async fn update_user(&self, user_id: u32, update_data: &Value) -> Result<User> {
        self.execute(
            Method::PATCH,
            &format!("/users/{}", user_id),
            Some(update_data),
        )
        .await
    }

    /// DELETE /users/{user_id} - Delete a user
    pub async fn delete_user(&self, user_id: u32) -> Result<()> {
        self.request(Method::DELETE, &format!("/users/{}", user_id), None::<&()>)
            .await?;
        Ok(())
    }

    /// GET /users/me - Get the current user
    pub async fn get_current_user(&self) -> Result<User> {
        self.execute(Method::GET, "/users/me", None::<&()>).await
    }

    /// GET /users?name={name} - Search users by name
    pub async fn search_users_by_name(&self, name: &str) -> Result<Vec<User>> {
        let params = [("name", name)];
        self.execute_with_params(Method::GET, "/users", None::<&()>, &params)
            .await
    }

    /// GET /users?email={email} - Search users by email
    pub async fn search_users_by_email(&self, email: &str) -> Result<Vec<User>> {
        let params = [("email", email)];
        self.execute_with_params(Method::GET, "/users", None::<&()>, &params)
            .await
    }

    /// GET /users/{user_id}/solves - Get solves for a user
    pub async fn get_user_solves(&self, user_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/users/{}/solves", user_id),
            None::<&()>,
        )
        .await
    }

    /// GET /users/{user_id}/fails - Get failures for a user
    pub async fn get_user_fails(&self, user_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/users/{}/fails", user_id),
            None::<&()>,
        )
        .await
    }

    /// GET /users/{user_id}/awards - Get awards for a user
    pub async fn get_user_awards(&self, user_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/users/{}/awards", user_id),
            None::<&()>,
        )
        .await
    }
}
