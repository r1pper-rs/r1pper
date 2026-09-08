use std::path::PathBuf;

/// Errors produced while configuring or running r1pper.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read configuration file {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse configuration file {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("could not write configuration file {path}: {source}")]
    WriteConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("could not read plugin directory {path}: {source}")]
    ReadPluginDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not read plugin manifest {path}: {source}")]
    ReadPluginManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse plugin manifest {path}: {source}")]
    ParsePluginManifest {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid plugin {plugin}: {message}")]
    InvalidPlugin { plugin: String, message: String },

    #[error("plugin {plugin} failed: {message}")]
    PluginRuntime { plugin: String, message: String },

    #[error("no plugin supports URL {0}")]
    UnsupportedUrl(String),

    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),

    #[error("could not create or write temporary download: {0}")]
    TemporaryFile(#[source] std::io::Error),

    #[error("could not create output directory {path}: {source}")]
    CreateOutputDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not finalize output file {path}: {source}")]
    FinalizeOutput {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not start ffmpeg at {path}: {source}")]
    StartFfmpeg {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(String),

    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, Error>;
