use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, auth::AuthCredentials};

/// Top-level settings loaded from TOML or assembled by an embedding application.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub output_directory: PathBuf,
    /// Relative template for the final output path.
    pub output_template: String,
    pub temporary_directory: Option<PathBuf>,
    pub plugin_directories: Vec<PathBuf>,
    pub concurrent_downloads: usize,
    pub overwrite: OverwritePolicy,
    pub audio: AudioProfile,
    /// Opaque per-plugin settings sent only to the matching plugin's `resolve` export.
    pub plugin_config: BTreeMap<String, serde_json::Value>,
    /// Per-plugin request settings supplied to plugins and applied to resulting media downloads.
    pub http: BTreeMap<String, HttpSettings>,
    /// Per-plugin credentials. Values are only supplied to the matching plugin.
    pub auth: BTreeMap<String, AuthCredentials>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_directory: PathBuf::from("downloads"),
            output_template: "{artist}/{album}/{track:02} - {title}.{ext}".into(),
            temporary_directory: None,
            plugin_directories: vec![PathBuf::from("plugins")],
            concurrent_downloads: 2,
            overwrite: OverwritePolicy::Rename,
            audio: AudioProfile::default(),
            plugin_config: BTreeMap::new(),
            http: BTreeMap::new(),
            auth: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpSettings {
    pub user_agent: Option<String>,
    pub headers: BTreeMap<String, String>,
}

impl Config {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path).map_err(|source| Error::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = toml::from_str(&contents).map_err(|source| Error::ParseConfig {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.concurrent_downloads == 0 {
            return Err(Error::InvalidConfig(
                "concurrent_downloads must be at least 1".into(),
            ));
        }
        let template = Path::new(&self.output_template);
        if self.output_template.trim().is_empty()
            || template.is_absolute()
            || template.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(Error::InvalidConfig(
                "output_template must be a non-empty relative path without parent directories"
                    .into(),
            ));
        }
        if !self.output_template.contains("{ext}") {
            return Err(Error::InvalidConfig(
                "output_template must contain the {ext} placeholder".into(),
            ));
        }
        self.audio.validate()
    }

    /// Stores credentials while preserving all other configuration values.
    pub fn save_auth_credentials(
        path: impl AsRef<Path>,
        plugin_id: &str,
        credentials: &AuthCredentials,
    ) -> Result<()> {
        let path = path.as_ref();
        let mut document = if path.exists() {
            let contents = fs::read_to_string(path).map_err(|source| Error::ReadConfig {
                path: path.to_path_buf(),
                source,
            })?;
            toml::from_str::<toml::Value>(&contents).map_err(|source| Error::ParseConfig {
                path: path.to_path_buf(),
                source,
            })?
        } else {
            toml::Value::Table(toml::map::Map::new())
        };
        let root = document.as_table_mut().ok_or_else(|| {
            Error::InvalidConfig("configuration root must be a TOML table".into())
        })?;
        let auth = root
            .entry("auth")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
            .as_table_mut()
            .ok_or_else(|| Error::InvalidConfig("auth must be a TOML table".into()))?;
        let mut credential_table = toml::map::Map::new();
        credential_table.insert(
            "scheme".into(),
            toml::Value::String(credentials.scheme.clone()),
        );
        for (key, value) in &credentials.values {
            credential_table.insert(key.clone(), toml::Value::String(value.clone()));
        }
        auth.insert(plugin_id.into(), toml::Value::Table(credential_table));

        let serialized =
            toml::to_string_pretty(&document).map_err(|error| Error::Message(error.to_string()))?;
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(path).map_err(|source| Error::WriteConfig {
            path: path.to_path_buf(),
            source,
        })?;
        file.write_all(serialized.as_bytes())
            .map_err(|source| Error::WriteConfig {
                path: path.to_path_buf(),
                source,
            })
    }
}

/// Settings passed to FFmpeg for an output file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioProfile {
    pub container: AudioContainer,
    pub codec: AudioCodec,
    pub bitrate_kbps: Option<u32>,
    pub quality: Option<u8>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u8>,
    pub normalize_loudness: bool,
    pub embed_artwork: bool,
}

impl Default for AudioProfile {
    fn default() -> Self {
        Self {
            container: AudioContainer::Opus,
            codec: AudioCodec::Opus,
            bitrate_kbps: Some(160),
            quality: None,
            sample_rate_hz: None,
            channels: None,
            normalize_loudness: false,
            embed_artwork: true,
        }
    }
}

impl AudioProfile {
    pub fn validate(&self) -> Result<()> {
        if self.bitrate_kbps.is_some() && self.quality.is_some() {
            return Err(Error::InvalidConfig(
                "audio.bitrate_kbps and audio.quality cannot both be set".into(),
            ));
        }
        if matches!(self.channels, Some(0)) {
            return Err(Error::InvalidConfig(
                "audio.channels must be at least 1".into(),
            ));
        }
        if matches!(self.sample_rate_hz, Some(0)) {
            return Err(Error::InvalidConfig(
                "audio.sample_rate_hz must be at least 1".into(),
            ));
        }
        let compatible = matches!(
            (self.container, self.codec),
            (AudioContainer::Mp3, AudioCodec::Mp3)
                | (AudioContainer::M4a, AudioCodec::Aac)
                | (AudioContainer::Opus, AudioCodec::Opus)
                | (AudioContainer::Flac, AudioCodec::Flac)
                | (AudioContainer::Wav, AudioCodec::PcmS16le)
        );
        if !compatible {
            return Err(Error::InvalidConfig(
                "audio.codec is not supported by audio.container".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverwritePolicy {
    Overwrite,
    Skip,
    #[default]
    Rename,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioContainer {
    Mp3,
    M4a,
    #[default]
    Opus,
    Flac,
    Wav,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Mp3,
    #[default]
    Aac,
    Opus,
    Flac,
    PcmS16le,
}
