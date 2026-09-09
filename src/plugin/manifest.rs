use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use extism::{Manifest, Plugin, Wasm};
use serde::Deserialize;
use url::Url;

use crate::{Config, Error, Result, auth::AuthScheme};

use super::{PLUGIN_API_VERSION, ResolveRequest, ResolveResponse, SearchRequest, SearchResponse};

#[derive(Clone, Debug)]
pub struct PluginDescriptor {
    pub id: String,
    pub version: String,
    pub url_patterns: Vec<String>,
    pub allowed_hosts: Vec<String>,
    pub auth_schemes: Vec<AuthScheme>,
    pub supports_search: bool,
    pub manifest_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ResolvedSource {
    pub plugin_id: String,
    pub response: ResolveResponse,
}

#[derive(Debug)]
struct PluginEntry {
    descriptor: PluginDescriptor,
    wasm_path: Option<PathBuf>,
}

/// Discovers plugins declared by `*.plugin.toml` manifests.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    entries: Vec<PluginEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginManifest {
    id: String,
    version: String,
    api_version: u32,
    wasm: PathBuf,
    url_patterns: Vec<String>,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default, rename = "auth")]
    auth_schemes: Vec<AuthScheme>,
    #[serde(default)]
    search: bool,
}

impl PluginRegistry {
    pub fn discover(config: &Config) -> Result<Self> {
        let mut entries = Vec::new();
        let mut ids = BTreeMap::new();

        for directory in &config.plugin_directories {
            if !directory.exists() {
                continue;
            }
            let dir_entries =
                fs::read_dir(directory).map_err(|source| Error::ReadPluginDirectory {
                    path: directory.clone(),
                    source,
                })?;
            for entry in dir_entries {
                let path = entry
                    .map_err(|source| Error::ReadPluginDirectory {
                        path: directory.clone(),
                        source,
                    })?
                    .path();
                if !path.to_string_lossy().ends_with(".plugin.toml") {
                    continue;
                }
                let parsed = Self::read_manifest(&path)?;
                if ids.insert(parsed.descriptor.id.clone(), ()).is_some() {
                    return Err(Error::InvalidPlugin {
                        plugin: parsed.descriptor.id,
                        message: "plugin ID is declared more than once".into(),
                    });
                }
                entries.push(parsed);
            }
        }
        if ids.insert("spotify".into(), ()).is_none() {
            entries.push(PluginEntry {
                descriptor: PluginDescriptor {
                    id: "spotify".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    url_patterns: vec![
                        "https://open.spotify.com/track/*".into(),
                        "https://open.spotify.com/album/*".into(),
                        "https://open.spotify.com/playlist/*".into(),
                        "spotify:track:*".into(),
                        "spotify:album:*".into(),
                        "spotify:playlist:*".into(),
                    ],
                    allowed_hosts: Vec::new(),
                    auth_schemes: vec![AuthScheme::OAuthDevice {
                        id: "spotify_device".into(),
                        device_code_url: "https://accounts.spotify.com/oauth2/device/authorize"
                            .into(),
                        token_url: "https://accounts.spotify.com/api/token".into(),
                        client_id: "65b708073fc0480ea92a077233ca87bd".into(),
                        client_secret: String::new(),
                        device_name: "r1pper Spotify Plugin".into(),
                    }],
                    supports_search: true,
                    manifest_path: PathBuf::from("<built-in>"),
                },
                wasm_path: None,
            });
        }
        if ids.insert("apple_music".into(), ()).is_none() {
            entries.push(PluginEntry {
                descriptor: PluginDescriptor {
                    id: "apple_music".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    url_patterns: vec!["https://music.apple.com/*".into()],
                    allowed_hosts: Vec::new(),
                    auth_schemes: Vec::new(),
                    supports_search: false,
                    manifest_path: PathBuf::from("<built-in>"),
                },
                wasm_path: None,
            });
        }
        if ids.insert("soulseek".into(), ()).is_none() {
            entries.push(PluginEntry {
                descriptor: PluginDescriptor {
                    id: "soulseek".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    url_patterns: vec!["soulseek://*".into()],
                    allowed_hosts: Vec::new(),
                    auth_schemes: Vec::new(),
                    supports_search: true,
                    manifest_path: PathBuf::from("<built-in>"),
                },
                wasm_path: None,
            });
        }
        Ok(Self { entries })
    }

    pub fn plugins(&self) -> impl Iterator<Item = &PluginDescriptor> {
        self.entries.iter().map(|entry| &entry.descriptor)
    }

    pub fn plugin(&self, id: &str) -> Option<&PluginDescriptor> {
        self.entries
            .iter()
            .find(|entry| entry.descriptor.id == id)
            .map(|entry| &entry.descriptor)
    }

    pub fn resolve(&self, url: &str, config: &Config) -> Result<ResolvedSource> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry_supports(entry, url))
            .ok_or_else(|| Error::UnsupportedUrl(url.into()))?;
        let wasm_path = entry
            .wasm_path
            .as_ref()
            .ok_or_else(|| Error::InvalidPlugin {
                plugin: entry.descriptor.id.clone(),
                message: "is a native plugin and must be resolved by the download pipeline".into(),
            })?;
        let wasm = fs::read(wasm_path).map_err(|source| Error::ReadPluginManifest {
            path: wasm_path.clone(),
            source,
        })?;
        let manifest = entry
            .descriptor
            .allowed_hosts
            .iter()
            .fold(Manifest::new([Wasm::data(wasm)]), |manifest, host| {
                manifest.with_allowed_host(host.clone())
            });
        let mut plugin =
            Plugin::new(manifest, [], false).map_err(|error| Error::PluginRuntime {
                plugin: entry.descriptor.id.clone(),
                message: error.to_string(),
            })?;
        let cfg = config
            .plugin_config
            .get(&entry.descriptor.id)
            .unwrap_or(&serde_json::Value::Null);
        let http = config
            .http
            .get(&entry.descriptor.id)
            .cloned()
            .unwrap_or_default();
        let auth = config.auth.get(&entry.descriptor.id);
        if let Some(auth) = auth
            && !entry
                .descriptor
                .auth_schemes
                .iter()
                .any(|scheme| scheme.id() == auth.scheme)
        {
            return Err(Error::InvalidConfig(format!(
                "auth.{} selects undeclared scheme {}",
                entry.descriptor.id, auth.scheme
            )));
        }
        let request = serde_json::to_string(&ResolveRequest {
            api_version: PLUGIN_API_VERSION,
            url,
            plugin_config: cfg,
            http: &http,
            auth,
        })
        .expect("the resolve request is serializable");
        let output: String =
            plugin
                .call("resolve", request)
                .map_err(|error| Error::PluginRuntime {
                    plugin: entry.descriptor.id.clone(),
                    message: error.to_string(),
                })?;
        let response: ResolveResponse =
            serde_json::from_str(&output).map_err(|error| Error::InvalidPlugin {
                plugin: entry.descriptor.id.clone(),
                message: format!("resolve returned invalid JSON: {error}"),
            })?;
        validate_response(&entry.descriptor.id, &response)?;
        Ok(ResolvedSource {
            plugin_id: entry.descriptor.id.clone(),
            response,
        })
    }

    /// Searches a plugin that declares the optional `search` export.
    pub fn search(
        &self,
        id: &str,
        query: &str,
        limit: usize,
        config: &Config,
    ) -> Result<Vec<crate::search::SearchResult>> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.descriptor.id == id)
            .ok_or_else(|| Error::Message(format!("plugin {id} is not installed")))?;
        if !entry.descriptor.supports_search {
            return Err(Error::Message(format!(
                "plugin {id} does not implement search"
            )));
        }
        let wasm_path = entry
            .wasm_path
            .as_ref()
            .ok_or_else(|| Error::Message(format!("plugin {id} is native")))?;
        let wasm = fs::read(wasm_path).map_err(|source| Error::ReadPluginManifest {
            path: wasm_path.clone(),
            source,
        })?;
        let manifest = entry
            .descriptor
            .allowed_hosts
            .iter()
            .fold(Manifest::new([Wasm::data(wasm)]), |manifest, host| {
                manifest.with_allowed_host(host.clone())
            });
        let mut plugin =
            Plugin::new(manifest, [], false).map_err(|error| Error::PluginRuntime {
                plugin: id.into(),
                message: error.to_string(),
            })?;
        let cfg = config
            .plugin_config
            .get(id)
            .unwrap_or(&serde_json::Value::Null);
        let http = config.http.get(id).cloned().unwrap_or_default();
        let auth = config.auth.get(id);
        let request = serde_json::to_string(&SearchRequest {
            api_version: PLUGIN_API_VERSION,
            query,
            limit,
            plugin_config: cfg,
            http: &http,
            auth,
        })
        .expect("the search request is serializable");
        let output: String =
            plugin
                .call("search", request)
                .map_err(|error| Error::PluginRuntime {
                    plugin: id.into(),
                    message: error.to_string(),
                })?;
        let response: SearchResponse =
            serde_json::from_str(&output).map_err(|error| Error::InvalidPlugin {
                plugin: id.into(),
                message: format!("search returned invalid JSON: {error}"),
            })?;
        if response.api_version != PLUGIN_API_VERSION {
            return Err(Error::InvalidPlugin {
                plugin: id.into(),
                message: "search returned an unsupported API version".into(),
            });
        }
        response
            .results
            .into_iter()
            .map(|result| {
                let parsed = Url::parse(&result.url).map_err(|_| Error::InvalidPlugin {
                    plugin: id.into(),
                    message: format!("search returned an invalid URL: {}", result.url),
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(Error::InvalidPlugin {
                        plugin: id.into(),
                        message: "search result URL must use HTTP or HTTPS".into(),
                    });
                }
                Ok(crate::search::SearchResult {
                    plugin_id: id.into(),
                    title: result.title,
                    url: result.url,
                    artist: result.artist,
                    album: result.album,
                    size: result.size,
                    artwork_url: result.artwork_url,
                })
            })
            .collect()
    }

    fn read_manifest(path: &Path) -> Result<PluginEntry> {
        let contents = fs::read_to_string(path).map_err(|source| Error::ReadPluginManifest {
            path: path.to_path_buf(),
            source,
        })?;
        let manifest: PluginManifest =
            toml::from_str(&contents).map_err(|source| Error::ParsePluginManifest {
                path: path.to_path_buf(),
                source,
            })?;
        if manifest.id.trim().is_empty() || manifest.url_patterns.is_empty() {
            return Err(Error::InvalidPlugin {
                plugin: manifest.id,
                message: "id and at least one URL pattern are required".into(),
            });
        }
        if manifest.api_version != PLUGIN_API_VERSION {
            return Err(Error::InvalidPlugin {
                plugin: manifest.id,
                message: format!(
                    "requires API {}, host supports {}",
                    manifest.api_version, PLUGIN_API_VERSION
                ),
            });
        }
        for host in &manifest.allowed_hosts {
            if host.trim().is_empty() || host.contains(['/', ':', ' ']) {
                return Err(Error::InvalidPlugin {
                    plugin: manifest.id,
                    message: format!("allowed host is invalid: {host}"),
                });
            }
        }
        let mut auth_ids = BTreeMap::new();
        for scheme in &manifest.auth_schemes {
            if scheme.id().trim().is_empty() || auth_ids.insert(scheme.id(), ()).is_some() {
                return Err(Error::InvalidPlugin {
                    plugin: manifest.id,
                    message: "authentication scheme IDs must be non-empty and unique".into(),
                });
            }
            let endpoints: Vec<&String> = match scheme {
                AuthScheme::OAuth {
                    authorization_url,
                    token_url,
                    redirect_uri,
                    ..
                } => vec![authorization_url, token_url, redirect_uri],
                AuthScheme::OAuthDevice {
                    device_code_url,
                    token_url,
                    ..
                } => vec![device_code_url, token_url],
                _ => Vec::new(),
            };
            for endpoint in endpoints {
                let parsed = Url::parse(endpoint).map_err(|_| Error::InvalidPlugin {
                    plugin: manifest.id.clone(),
                    message: format!("OAuth endpoint is invalid: {endpoint}"),
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(Error::InvalidPlugin {
                        plugin: manifest.id.clone(),
                        message: format!("OAuth endpoint must use HTTP or HTTPS: {endpoint}"),
                    });
                }
            }
        }
        let wasm_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(manifest.wasm);
        if !wasm_path.is_file() {
            return Err(Error::InvalidPlugin {
                plugin: manifest.id,
                message: format!("WASM module does not exist: {}", wasm_path.display()),
            });
        }
        Ok(PluginEntry {
            descriptor: PluginDescriptor {
                id: manifest.id,
                version: manifest.version,
                url_patterns: manifest.url_patterns,
                allowed_hosts: manifest.allowed_hosts,
                auth_schemes: manifest.auth_schemes,
                supports_search: manifest.search,
                manifest_path: path.to_path_buf(),
            },
            wasm_path: Some(wasm_path),
        })
    }
}

fn entry_supports(entry: &PluginEntry, url: &str) -> bool {
    entry
        .descriptor
        .url_patterns
        .iter()
        .any(|pattern| glob_matches(pattern, url))
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let mut remainder = value;
    for (index, part) in pattern.split('*').enumerate() {
        if part.is_empty() {
            continue;
        }
        let Some(position) = remainder.find(part) else {
            return false;
        };
        if index == 0 && !pattern.starts_with('*') && position != 0 {
            return false;
        }
        remainder = &remainder[position + part.len()..];
    }
    pattern.ends_with('*') || remainder.is_empty()
}

fn validate_response(plugin: &str, response: &ResolveResponse) -> Result<()> {
    if response.api_version != PLUGIN_API_VERSION {
        return Err(Error::InvalidPlugin {
            plugin: plugin.into(),
            message: "resolve returned an unsupported API version".into(),
        });
    }
    if response.candidates.is_empty()
        && response.items.is_empty()
        && response.deferred_urls.is_empty()
    {
        return Err(Error::InvalidPlugin {
            plugin: plugin.into(),
            message: "resolve returned no media candidates".into(),
        });
    }
    for candidate in response.candidates.iter().chain(
        response
            .items
            .iter()
            .flat_map(|item| item.candidates.iter()),
    ) {
        let parsed = Url::parse(&candidate.url).map_err(|_| Error::InvalidPlugin {
            plugin: plugin.into(),
            message: format!("candidate URL is invalid: {}", candidate.url),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::InvalidPlugin {
                plugin: plugin.into(),
                message: "candidate URL must use HTTP or HTTPS".into(),
            });
        }
        if candidate
            .headers
            .keys()
            .any(|name| name.trim().is_empty() || name.contains(['\r', '\n', ':']))
            || candidate
                .headers
                .values()
                .any(|value| value.contains(['\r', '\n']))
        {
            return Err(Error::InvalidPlugin {
                plugin: plugin.into(),
                message: "candidate headers contain an invalid name or value".into(),
            });
        }
    }
    if response.items.iter().any(|item| item.candidates.is_empty()) {
        return Err(Error::InvalidPlugin {
            plugin: plugin.into(),
            message: "each resolved item must contain a media candidate".into(),
        });
    }
    for metadata in
        std::iter::once(&response.metadata).chain(response.items.iter().map(|item| &item.metadata))
    {
        if let Some(artwork_url) = &metadata.artwork_url {
            let artwork = Url::parse(artwork_url).map_err(|_| Error::InvalidPlugin {
                plugin: plugin.into(),
                message: format!("artwork URL is invalid: {artwork_url}"),
            })?;
            if !matches!(artwork.scheme(), "http" | "https") {
                return Err(Error::InvalidPlugin {
                    plugin: plugin.into(),
                    message: "artwork URL must use HTTP or HTTPS".into(),
                });
            }
        }
    }
    for url in &response.deferred_urls {
        let parsed = Url::parse(url).map_err(|_| Error::InvalidPlugin {
            plugin: plugin.into(),
            message: format!("deferred URL is invalid: {url}"),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::InvalidPlugin {
                plugin: plugin.into(),
                message: "deferred URL must use HTTP or HTTPS".into(),
            });
        }
    }
    Ok(())
}
