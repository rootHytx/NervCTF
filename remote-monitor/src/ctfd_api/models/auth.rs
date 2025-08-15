use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct LoginRequest {
    pub name: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    pub user_id: u32,
    pub name: String,
    pub email: String,
    // Add other fields as needed from CTFd API response
}
