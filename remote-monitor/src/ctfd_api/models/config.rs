use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigItem {
    pub id: u32,
    pub key: String,
    pub value: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub type_field: Option<String>,
    pub category: Option<String>,
    pub editable: bool,
    pub required: bool,
    pub public: bool,
}

#[derive(Debug, Serialize)]
pub struct ConfigUpdate {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfigCategory {
    pub name: String,
    pub configs: Vec<ConfigItem>,
}
