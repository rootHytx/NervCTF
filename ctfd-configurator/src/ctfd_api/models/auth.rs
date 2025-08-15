use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct LoginRequest {
    pub name: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    pub user_id: Option<u32>,
    pub name: Option<String>,
    pub email: Option<String>,
    // Add other fields as needed from CTFd API response
}
