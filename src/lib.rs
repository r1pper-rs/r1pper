//! Reusable audio download pipeline and source-plugin host.

pub mod apple_music;
pub mod audio;
pub mod auth;
pub mod config;
pub mod download;
pub mod error;
pub mod output;
pub mod pipeline;
pub mod plugin;
pub mod search;
pub mod soulseek;
pub mod spotify;

pub use config::{AudioProfile, Config, OverwritePolicy};
pub use error::{Error, Result};
pub use pipeline::{
    DownloadBatchResult, DownloadEvent, DownloadRequest, DownloadResult, Downloader,
};
