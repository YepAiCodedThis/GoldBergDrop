use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteamApp {
    pub app_id: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlcApp {
    pub app_id: u32,
    pub name: String,
}

/// Store search response from `store.steampowered.com/api/storesearch`.
#[derive(Debug, Deserialize)]
pub struct StoreSearchResponse {
    #[serde(default)]
    pub items: Vec<StoreSearchItem>,
}

#[derive(Debug, Deserialize)]
pub struct StoreSearchItem {
    #[serde(rename = "type")]
    pub item_type: String,
    pub id: u32,
    pub name: String,
}

/// Response wrapper from `store.steampowered.com/api/appdetails`.
#[derive(Debug, Deserialize)]
pub struct AppDetailsEnvelope {
    pub success: bool,
    #[serde(default)]
    pub data: Option<AppDetailsData>,
}

#[derive(Debug, Deserialize)]
pub struct AppDetailsData {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub dlc: Vec<u32>,
}
