use crate::ctfd_api::CtfdClient;
use crate::ctfd_api::models::pages::Page;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// GET /pages - List all pages
    pub async fn get_pages(&self) -> Result<Vec<Page>> {
        self.execute(Method::GET, "/pages", None::<&()>).await
    }

    /// GET /pages/{page_id} - Get a specific page
    pub async fn get_page(&self, page_id: u32) -> Result<Page> {
        self.execute(Method::GET, &format!("/pages/{}", page_id), None::<&()>)
            .await
    }

    /// POST /pages - Create a new page
    pub async fn create_page(&self, page_data: &Value) -> Result<Page> {
        self.execute(Method::POST, "/pages", Some(page_data)).await
    }

    /// PATCH /pages/{page_id} - Update a page
    pub async fn update_page(&self, page_id: u32, update_data: &Value) -> Result<Page> {
        self.execute(
            Method::PATCH,
            &format!("/pages/{}", page_id),
            Some(update_data),
        )
        .await
    }

    /// DELETE /pages/{page_id} - Delete a page
    pub async fn delete_page(&self, page_id: u32) -> Result<()> {
        self.request(Method::DELETE, &format!("/pages/{}", page_id), None::<&()>)
            .await?;
        Ok(())
    }

    /// GET /pages?route={route} - Get a page by its route
    pub async fn get_page_by_route(&self, route: &str) -> Result<Page> {
        let params = [("route", route)];
        self.execute_with_params(Method::GET, "/pages", None::<&()>, &params)
            .await
    }
}
