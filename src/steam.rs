use crate::models::{AppDetailsEnvelope, DlcApp, SteamApp, StoreSearchResponse};
use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

const USER_AGENT: &str = "GoldbergDrop/0.1";

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()
        .context("Failed to build HTTP client")
}

/// Builds the ordered list of search terms to try: exe name, folder name, full path.
pub fn build_search_terms(exe_path: &Path) -> Vec<String> {
    let mut terms = Vec::new();

    if let Some(stem) = exe_path.file_stem().and_then(|s| s.to_str()) {
        if !stem.trim().is_empty() {
            terms.push(stem.to_string());
        }
    }

    if let Some(folder_name) = exe_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
    {
        if !folder_name.trim().is_empty()
            && !terms
                .iter()
                .any(|t| t.eq_ignore_ascii_case(folder_name))
        {
            terms.push(folder_name.to_string());
        }
    }

    if let Some(full) = exe_path.to_str() {
        if !terms.iter().any(|t| t.eq_ignore_ascii_case(full)) {
            terms.push(full.to_string());
        }
    }

    terms
}

/// Searches the Steam store for apps matching the given term.
/// Returns only items of type "app" (games), not DLC/bundles/etc.
pub fn store_search(term: &str) -> Result<Vec<SteamApp>> {
    if term.trim().is_empty() {
        return Ok(Vec::new());
    }

    let client = client()?;
    let url = "https://store.steampowered.com/api/storesearch/";
    let response = client
        .get(url)
        .query(&[("term", term), ("cc", "US"), ("l", "english")])
        .send()
        .context("Store search request failed")?;

    let body: StoreSearchResponse = response
        .json()
        .context("Failed to parse store search response")?;

    Ok(body
        .items
        .into_iter()
        .filter(|item| item.item_type.eq_ignore_ascii_case("app"))
        .map(|item| SteamApp {
            app_id: item.id,
            name: item.name,
        })
        .collect())
}

/// Tries to find a single Steam app for the given executable path by trying
/// progressively less specific search terms (exe name, folder name, full path).
/// Returns `Ok(Some(app))` on a confident single/exact match, `Ok(None)` if no
/// terms produced any results, or the full candidate list via `store_search`
/// for the caller to present a picker when a term is ambiguous.
pub fn find_app_by_exe(exe_path: &Path) -> Result<AppSearchOutcome> {
    let terms = build_search_terms(exe_path);

    for term in &terms {
        let results = store_search(term)?;
        match results.len() {
            0 => continue,
            1 => return Ok(AppSearchOutcome::Found(results[0].clone())),
            _ => {
                if let Some(exact) = results
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(term))
                {
                    return Ok(AppSearchOutcome::Found(exact.clone()));
                }
                return Ok(AppSearchOutcome::Ambiguous(results));
            }
        }
    }

    Ok(AppSearchOutcome::NotFound)
}

pub enum AppSearchOutcome {
    Found(SteamApp),
    Ambiguous(Vec<SteamApp>),
    NotFound,
}

/// Fetches basic app info via the app details endpoint. Used to validate a
/// manually entered App ID and to resolve its display name.
pub fn get_app_name(app_id: u32) -> Result<Option<String>> {
    let envelope = fetch_app_details(app_id)?;
    Ok(envelope.and_then(|e| e.data).and_then(|d| d.name))
}

/// Fetches the DLC app IDs for a given base game App ID, then resolves each
/// DLC's display name via the app details endpoint (falling back to a
/// generic "Unknown DLC {id}" label when no name can be found).
pub fn get_dlc_list(app_id: u32) -> Result<Vec<DlcApp>> {
    let data = match fetch_app_details(app_id)?.and_then(|e| e.data) {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };

    let mut dlc_list = Vec::with_capacity(data.dlc.len());
    for dlc_id in data.dlc {
        let name = get_app_name(dlc_id)
            .ok()
            .flatten()
            .unwrap_or_else(|| format!("Unknown DLC {dlc_id}"));
        dlc_list.push(DlcApp {
            app_id: dlc_id,
            name,
        });
    }

    Ok(dlc_list)
}

/// Fetches and parses the `appdetails` envelope for a single App ID.
/// Returns `Ok(None)` if the API reports `success: false` for this ID.
fn fetch_app_details(app_id: u32) -> Result<Option<AppDetailsEnvelope>> {
    let client = client()?;
    let url = "https://store.steampowered.com/api/appdetails";
    let response = client
        .get(url)
        .query(&[("appids", app_id.to_string()), ("l", "english".into())])
        .send()
        .context("App details request failed")?;

    let body: serde_json::Value = response.json().context("Failed to parse app details")?;
    let entry = match body.get(app_id.to_string()) {
        Some(v) => v.clone(),
        None => return Ok(None),
    };
    let envelope: AppDetailsEnvelope =
        serde_json::from_value(entry).context("Failed to parse app details envelope")?;

    if !envelope.success {
        return Ok(None);
    }

    Ok(Some(envelope))
}
