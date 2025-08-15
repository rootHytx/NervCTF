use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigItem {
    pub id: Option<u32>,
    pub key: Option<String>,
    pub value: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub type_field: Option<String>,
    pub category: Option<String>,
    pub editable: Option<bool>,
    pub required: Option<bool>,
    pub public: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ConfigUpdate {
    pub value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConfigCategory {
    pub name: Option<String>,
    pub configs: Option<Vec<ConfigItem>>,
}
