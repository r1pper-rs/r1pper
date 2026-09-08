use std::path::PathBuf;

use std::collections::BTreeMap;

use crate::OverwritePolicy;
use crate::{
    Config, Result,
    audio::FfmpegRunner,
    download,
    output::{self, OutputTarget},
    plugin::{PluginRegistry, ResolvedItem, SourceMetadata},
};

#[derive(Clone, Debug)]
pub struct DownloadRequest {
    pub url: String,
    /// Overrides the configured collision behavior for this invocation.
    pub overwrite: Option<OverwritePolicy>,
}

#[derive(Clone, Debug)]
pub struct DownloadResult {
    pub plugin_id: String,
    pub output_path: PathBuf,
    pub skipped: bool,
    pub metadata: SourceMetadata,
}

#[derive(Clone, Debug)]
pub struct DownloadBatchResult {
    pub plugin_id: String,
    pub downloads: Vec<DownloadResult>,
}

#[derive(Clone, Debug)]
pub enum DownloadEvent {
    Resolving,
    StartingItem {
        index: usize,
        total: usize,
        title: Option<String>,
    },
    Downloading {
        downloaded: u64,
        total: Option<u64>,
    },
    Processing,
    Finished(PathBuf),
    Skipped(PathBuf),
}

pub struct Downloader {
    config: Config,
    plugins: PluginRegistry,
    ffmpeg: FfmpegRunner,
}

impl Downloader {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        let plugins = PluginRegistry::discover(&config)?;
        Ok(Self {
            config,
            plugins,
            ffmpeg: FfmpegRunner::default(),
        })
    }

    pub fn with_ffmpeg(mut self, ffmpeg: FfmpegRunner) -> Self {
        self.ffmpeg = ffmpeg;
        self
    }

    pub fn download(&self, request: DownloadRequest) -> Result<DownloadResult> {
        let batch = self.download_all_with_progress(request, &mut |_| {})?;
        if batch.downloads.len() != 1 {
            return Err(crate::Error::Message(
                "source resolved to multiple items; call download_all_with_progress instead".into(),
            ));
        }
        Ok(batch
            .downloads
            .into_iter()
            .next()
            .expect("batch has one item"))
    }

    pub fn download_all_with_progress(
        &self,
        request: DownloadRequest,
        progress: &mut dyn FnMut(DownloadEvent),
    ) -> Result<DownloadBatchResult> {
        self.ffmpeg.verify()?;
        let overwrite = request.overwrite.unwrap_or(self.config.overwrite);
        if crate::spotify::supports(&request.url) {
            return crate::spotify::download(
                &self.config,
                &self.ffmpeg,
                &request.url,
                overwrite,
                progress,
            );
        }
        if crate::soulseek::supports(&request.url) {
            return crate::soulseek::download(
                &self.config,
                &self.ffmpeg,
                &request.url,
                overwrite,
                progress,
            );
        }
        progress(DownloadEvent::Resolving);
        let resolved = self.plugins.resolve(&request.url, &self.config)?;
        let http = self
            .config
            .http
            .get(&resolved.plugin_id)
            .cloned()
            .unwrap_or_default();
        let deferred = resolved.response.deferred_urls;
        let items =
            if resolved.response.items.is_empty() && !resolved.response.candidates.is_empty() {
                vec![ResolvedItem {
                    candidates: resolved.response.candidates,
                    metadata: resolved.response.metadata,
                }]
            } else {
                resolved.response.items
            };
        let total = items.len() + deferred.len();
        let mut downloads = Vec::with_capacity(total);
        for (index, item) in items.into_iter().enumerate() {
            progress(DownloadEvent::StartingItem {
                index: index + 1,
                total,
                title: item.metadata.title.clone(),
            });
            downloads.push(self.download_item(
                &resolved.plugin_id,
                &http,
                item,
                overwrite,
                progress,
            )?);
        }
        let completed = downloads.len();
        for (offset, url) in deferred.into_iter().enumerate() {
            let index = completed + offset + 1;
            progress(DownloadEvent::StartingItem {
                index,
                total,
                title: Some("Resolving track...".into()),
            });
            let nested = self.plugins.resolve(&url, &self.config)?;
            if !nested.response.items.is_empty() || !nested.response.deferred_urls.is_empty() {
                return Err(crate::Error::Message(
                    "a deferred source URL must resolve to exactly one media item".into(),
                ));
            }
            let item = ResolvedItem {
                candidates: nested.response.candidates,
                metadata: nested.response.metadata,
            };
            progress(DownloadEvent::StartingItem {
                index,
                total,
                title: item.metadata.title.clone(),
            });
            downloads.push(self.download_item(
                &nested.plugin_id,
                &http,
                item,
                overwrite,
                progress,
            )?);
        }
        Ok(DownloadBatchResult {
            plugin_id: resolved.plugin_id,
            downloads,
        })
    }

    fn download_item(
        &self,
        plugin_id: &str,
        http: &crate::config::HttpSettings,
        item: ResolvedItem,
        overwrite: OverwritePolicy,
        progress: &mut dyn FnMut(DownloadEvent),
    ) -> Result<DownloadResult> {
        let target = output::target(
            &self.config.output_directory,
            &self.config.output_template,
            &item.metadata,
            &self.config.audio,
            overwrite,
        )?;
        let (destination, skipped) = match target {
            OutputTarget::Write(path) => (path, false),
            OutputTarget::Skip(path) => (path, true),
        };
        if skipped {
            progress(DownloadEvent::Skipped(destination.clone()));
            return Ok(DownloadResult {
                plugin_id: plugin_id.into(),
                output_path: destination,
                skipped,
                metadata: item.metadata,
            });
        }
        let source = {
            let mut report = |downloaded, total| {
                progress(DownloadEvent::Downloading { downloaded, total });
            };
            download::fetch(
                &item.candidates[0],
                self.config.temporary_directory.as_deref(),
                &http,
                Some(&mut report),
            )?
        };
        let artwork = if self.config.audio.embed_artwork {
            item.metadata
                .artwork_url
                .as_ref()
                .map(|url| {
                    download::fetch(
                        &crate::plugin::MediaCandidate {
                            url: url.clone(),
                            headers: BTreeMap::new(),
                            mime_type: None,
                            codec: None,
                        },
                        self.config.temporary_directory.as_deref(),
                        &http,
                        None,
                    )
                })
                .transpose()?
        } else {
            None
        };
        progress(DownloadEvent::Processing);
        self.ffmpeg.transcode(
            source.path(),
            &destination,
            &self.config.audio,
            &item.metadata,
            artwork.as_ref().map(|file| file.path()),
        )?;
        progress(DownloadEvent::Finished(destination.clone()));
        Ok(DownloadResult {
            plugin_id: plugin_id.into(),
            output_path: destination,
            skipped,
            metadata: item.metadata,
        })
    }
}
