//! "Browse connectors": a curated list of vendor-hosted remote MCP servers,
//! shipped in the binary and never fetched (a list from the network would be
//! a trust decision made by whoever serves it). Each URL is the one the
//! vendor's own documentation (`docs_url`) gives, checked 2026-11-22; the
//! dialog adds one with the custom-connector form, its domain shown first.

use crate::error::AppResult;
use crate::state::AppState;
use serde_json::{json, Value};
use std::sync::Arc;

const LIST: &str = include_str!("connector_directory.json");

pub const AUTHS: [&str; 3] = ["oauth", "bearer", "none"];
pub const CATEGORIES: [&str; 13] = ["dev", "cloud", "projects", "docs", "search", "payments", "crm", "support", "design", "data", "analytics", "media", "automation"];

/// `u` as compared with a configured server's URL: parsed (scheme and host in
/// lower case, default port dropped), without a trailing slash.
fn normalized(u: &str) -> Option<String> {
    url::Url::parse(u.trim()).ok().map(|u| u.as_str().trim_end_matches('/').to_string())
}

/// The directory, each entry with `configured`: the name of the server in
/// `config` at its URL, or null.
pub fn entries(config: &Value) -> Vec<Value> {
    let mut list: Vec<Value> = serde_json::from_str(LIST).unwrap_or_default();
    let configured: Vec<(String, String)> = config["mcpServers"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, s)| Some((name.clone(), normalized(s["url"].as_str()?)?)))
        .collect();
    for e in &mut list {
        let url = e["url"].as_str().and_then(normalized);
        let name = configured.iter().find(|(_, u)| Some(u) == url.as_ref()).map(|(n, _)| n.clone());
        e["configured"] = json!(name);
    }
    list
}

#[tauri::command]
pub async fn connector_directory(state: tauri::State<'_, Arc<AppState>>) -> AppResult<Value> {
    Ok(Value::Array(entries(&crate::mcp::read_config(&state.mcp.config_path))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_entry_is_complete_https_without_credentials_and_unique() {
        let list: Vec<Value> = serde_json::from_str(LIST).expect("the list is valid JSON");
        assert!(list.len() >= 40, "{} entries", list.len());
        let (mut names, mut urls) = (HashSet::new(), HashSet::new());
        for e in &list {
            let name = e["name"].as_str().unwrap_or_default();
            assert!(!name.trim().is_empty() && names.insert(name.to_lowercase()), "name {name:?} empty or twice");
            let raw = e["url"].as_str().unwrap_or_default();
            let u = url::Url::parse(raw).unwrap_or_else(|err| panic!("{name}: {raw}: {err}"));
            assert_eq!(u.scheme(), "https", "{name}");
            assert!(u.username().is_empty() && u.password().is_none(), "{name}: credentials in the URL");
            assert!(u.query().is_none() && u.fragment().is_none(), "{name}: a query can carry a key");
            assert_eq!(u.host_str(), e["domain"].as_str(), "{name}: the domain shown is the URL's host");
            assert!(urls.insert(normalized(raw).unwrap()), "{name}: URL twice");
            assert!(AUTHS.contains(&e["auth"].as_str().unwrap_or_default()), "{name}: auth");
            assert!(CATEGORIES.contains(&e["category"].as_str().unwrap_or_default()), "{name}: category");
            let docs = url::Url::parse(e["docs_url"].as_str().unwrap_or_default()).unwrap_or_else(|err| panic!("{name}: docs_url: {err}"));
            assert_eq!(docs.scheme(), "https", "{name}: docs_url");
            for lang in ["en", "tr"] {
                assert!(!e["description"][lang].as_str().unwrap_or_default().trim().is_empty(), "{name}: description.{lang}");
            }
        }
    }

    #[test]
    fn every_category_has_entries_and_a_label_in_both_languages() {
        let list: Vec<Value> = serde_json::from_str(LIST).unwrap();
        let i18n = include_str!("../../ui/js/i18n.js");
        for c in CATEGORIES {
            assert!(list.iter().any(|e| e["category"] == c), "{c}: no entry");
            assert_eq!(i18n.matches(&format!("cd_cat_{c}:")).count(), 2, "{c}: cd_cat_{c} in English and Turkish");
        }
    }

    #[test]
    fn entries_already_configured_name_their_server() {
        let config = json!({"mcpServers": {
            "work tracker": {"url": "HTTPS://MCP.LINEAR.APP/mcp/"},
            "local": {"command": "npx"},
            "odd": {"url": "not a url"},
        }});
        let list = entries(&config);
        let by = |n: &str| list.iter().find(|e| e["name"] == n).unwrap()["configured"].clone();
        assert_eq!(by("Linear"), json!("work tracker"));
        assert_eq!(by("Notion"), Value::Null);
        assert!(list.iter().filter(|e| !e["configured"].is_null()).count() == 1);
        // a broken config marks nothing and still lists everything
        assert_eq!(entries(&json!({"mcpServers": []})).len(), list.len());
    }
}
