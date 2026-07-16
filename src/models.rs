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

/// GGNetwork `steam.item` lookup response.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GgItemResponse {
    pub result: u32,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub game: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub download: Option<String>,
}

impl GgItemResponse {
    pub fn is_available(&self) -> bool {
        self.result == 1
    }

    pub fn game_app_id(&self) -> Option<u32> {
        self.game.parse().ok()
    }
}

/// Parsed row from `ISteamRemoteStorage/GetPublishedFileDetails` (no API key).
#[derive(Debug, Clone)]
pub struct PublishedFileMeta {
    pub title: String,
    pub app_id: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PublishedFileDetailsEnvelope {
    pub(crate) response: PublishedFileDetailsResponse,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PublishedFileDetailsResponse {
    #[serde(default)]
    pub(crate) publishedfiledetails: Vec<PublishedFileDetailsItem>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PublishedFileDetailsItem {
    #[serde(default)]
    pub(crate) publishedfileid: String,
    #[serde(default)]
    pub(crate) result: u32,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) consumer_app_id: u32,
    #[serde(default)]
    pub(crate) creator_app_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueStatus {
    Queued,
    Downloading,
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub workshop_id: u64,
    pub mod_name: String,
    pub game_app_id: u32,
    pub game_name: String,
    pub status: QueueStatus,
    /// Filled in after a successful GGNetwork lookup, needed for the download step.
    pub gg_item: Option<GgItemResponse>,
    /// `None` while resolving; `Some(false)` = not cached on GGNetwork (show `!`).
    pub gg_available: Option<bool>,
}
