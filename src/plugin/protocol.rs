use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{auth::AuthCredentials, config::HttpSettings};

/// The JSON ABI version implemented by the host and source plugins.
pub const PLUGIN_API_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize)]
pub struct ResolveRequest<'a> {
    pub api_version: u32,
    pub url: &'a str,
    pub plugin_config: &'a serde_json::Value,
    pub http: &'a HttpSettings,
    pub auth: Option<&'a AuthCredentials>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchRequest<'a> {
    pub api_version: u32,
    pub query: &'a str,
    pub limit: usize,
    pub plugin_config: &'a serde_json::Value,
    pub http: &'a HttpSettings,
    pub auth: Option<&'a AuthCredentials>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchResponse {
    pub api_version: u32,
    #[serde(default)]
    pub results: Vec<SearchResult>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub size: Option<u64>,
    #[serde(default)]
    pub artwork_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveResponse {
    pub api_version: u32,
    #[serde(default)]
    pub candidates: Vec<MediaCandidate>,
    #[serde(default)]
    pub metadata: SourceMetadata,
    #[serde(default)]
    pub items: Vec<ResolvedItem>,
    /// Source URLs the host resolves lazily, one at a time, for large collections.
    #[serde(default)]
    pub deferred_urls: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedItem {
    pub candidates: Vec<MediaCandidate>,
    #[serde(default)]
    pub metadata: SourceMetadata,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaCandidate {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub mime_type: Option<String>,
    pub codec: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub date: Option<String>,
    pub genre: Option<String>,
    pub artwork_url: Option<String>,
}
