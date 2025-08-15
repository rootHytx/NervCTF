use anyhow::{anyhow, Result};
use reqwest::{header, Client, Method, Response};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;

const BASE_PATH: &str = "/api/v1";
const DEFAULT_TIMEOUT: u64 = 10;

/// Main client for interacting with the CTFd API
#[derive(Clone)]
pub struct CtfdClient {
    client: Client,
    base_url: String,
    api_key: String,
}

impl CtfdClient {
    /// Creates a new CTFd client instance
    ///
    /// # Arguments
    /// * `base_url` - Base URL of the CTFd instance (e.g., "https://ctfd.example.com")
    /// * `api_key` - API key for authentication
    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "Authorization",
            header::HeaderValue::from_str(&format!("Token {}", api_key))
                .map_err(|e| anyhow!("Invalid API key: {}", e))?,
        );
        headers.insert(
            "Content-Type",
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT))
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        })
    }

    /// Executes an API request
    pub async fn request<T: Serialize + ?Sized>(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&T>,
    ) -> Result<Response> {
        let url = format!("{}{}{}", self.base_url, BASE_PATH, endpoint);
        let mut builder = self.client.request(method.clone(), &url);

        if let Some(body) = body {
            builder = builder.json(body);
        }

        let response = builder.send().await?;
        let status = response.status();

        if status.is_success() {
            Ok(response)
        } else {
            let error_text = response.text().await?;
            Err(anyhow!(
                "API error ({} {}): {}",
                method,
                endpoint,
                error_text
            ))
        }
    }

    /// Parses the API response
    pub async fn parse_response<T: DeserializeOwned>(response: Response) -> Result<T> {
        let json = response.json::<ApiResponse<T>>().await?;
        if json.success {
            json.data.ok_or_else(|| anyhow!("Missing data in response"))
        } else {
            Err(anyhow!("API error: {}", json.message.unwrap_or_default()))
        }
    }

    /// Executes a request and parses the response
    pub async fn execute<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let response = self.request(method, endpoint, body).await?;
        Self::parse_response(response).await
    }

    /// Executes a request with query parameters and parses the response
    pub async fn execute_with_params<T: DeserializeOwned, B: Serialize + ?Sized, P: Serialize>(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&B>,
        params: &P,
    ) -> Result<T> {
        let url = format!("{}{}{}", self.base_url, BASE_PATH, endpoint);
        let mut builder = self.client.request(method.clone(), &url).query(params);

        if let Some(body) = body {
            builder = builder.json(body);
        }

        let response = builder.send().await?;
        let status = response.status();

        if status.is_success() {
            Self::parse_response(response).await
        } else {
            let error_text = response.text().await?;
            Err(anyhow!(
                "API error ({} {}): {}",
                method,
                endpoint,
                error_text
            ))
        }
    }

    /// Executes a request without expecting a response body
    pub async fn request_without_body<T: Serialize + ?Sized>(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&T>,
    ) -> Result<()> {
        self.request(method, endpoint, body).await?;
        Ok(())
    }
}

/// Generic API response structure
#[derive(Debug, serde::Deserialize)]
struct ApiResponse<T> {
    success: bool,
    message: Option<String>,
    data: Option<T>,
}
