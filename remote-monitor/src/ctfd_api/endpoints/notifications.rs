use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Notification {
    pub id: u32,
    pub title: String,
    pub content: String,
    pub user_id: Option<u32>,
    pub team_id: Option<u32>,
    pub date: String,
    pub html: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct NotificationCreate {
    pub title: String,
    pub content: String,
    pub user_id: Option<u32>,
    pub team_id: Option<u32>,
    pub html: Option<bool>,
}

impl CtfdClient {
    /// GET /notifications - List all notifications
    pub async fn get_notifications(&self) -> Result<Vec<Notification>> {
        self.execute(Method::GET, "/notifications", None::<&()>)
            .await
    }

    /// GET /notifications/{notification_id} - Get a specific notification
    pub async fn get_notification(&self, notification_id: u32) -> Result<Notification> {
        self.execute(
            Method::GET,
            &format!("/notifications/{}", notification_id),
            None::<&()>,
        )
        .await
    }

    /// POST /notifications - Create a new notification
    pub async fn create_notification(
        &self,
        notification: &NotificationCreate,
    ) -> Result<Notification> {
        self.execute(Method::POST, "/notifications", Some(notification))
            .await
    }

    /// DELETE /notifications/{notification_id} - Delete a notification
    pub async fn delete_notification(&self, notification_id: u32) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/notifications/{}", notification_id),
            None::<&()>,
        )
        .await?;
        Ok(())
    }
}
