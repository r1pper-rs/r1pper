use std::time::Duration;

use soulseek_rs::{Client, ClientSettings, DownloadStatus};

use crate::{
    OverwritePolicy,
    audio::FfmpegRunner,
    config::Config,
    error::{Error, Result},
    output::{self, OutputTarget},
    pipeline::{DownloadBatchResult, DownloadEvent, DownloadResult},
    plugin::SourceMetadata,
    search::SearchResult,
};

const SEARCH_WINDOW: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub fn supports(url: &str) -> bool {
    parse_url(url).is_some()
}

pub fn search(config: &Config, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let client = match connect(config) {
        Ok(client) => client,
        Err(error) if is_connection_error(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let results = client.search(query, SEARCH_WINDOW).map_err(client_error)?;
    let mut files: Vec<_> = results
        .into_iter()
        .flat_map(|result| result.files)
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.size));
    Ok(files
        .into_iter()
        .take(limit)
        .map(|file| SearchResult {
            plugin_id: "soulseek".into(),
            title: file
                .name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&file.name)
                .to_string(),
            url: format!(
                "soulseek://{}?path={}&size={}",
                file.username,
                url::form_urlencoded::byte_serialize(file.name.as_bytes()).collect::<String>(),
                file.size
            ),
            artist: Some(file.username),
            album: None,
            size: Some(file.size),
            artwork_url: None,
        })
        .collect())
}

pub fn download(
    config: &Config,
    ffmpeg: &FfmpegRunner,
    url: &str,
    overwrite: OverwritePolicy,
    progress: &mut dyn FnMut(DownloadEvent),
) -> Result<DownloadBatchResult> {
    let (username, filename, size) =
        parse_url(url).ok_or_else(|| Error::UnsupportedUrl(url.into()))?;
    let metadata = SourceMetadata {
        title: filename.rsplit(['/', '\\']).next().map(str::to_owned),
        artist: Some(username.to_owned()),
        album: parent_folder(&filename),
        ..SourceMetadata::default()
    };
    progress(DownloadEvent::Resolving);
    progress(DownloadEvent::StartingItem {
        index: 1,
        total: 1,
        title: metadata.title.clone(),
    });
    let target = output::target(
        &config.output_directory,
        &config.output_template,
        &metadata,
        &config.audio,
        overwrite,
    )?;
    let (destination, skipped) = match target {
        OutputTarget::Write(path) => (path, false),
        OutputTarget::Skip(path) => (path, true),
    };
    if skipped {
        progress(DownloadEvent::Skipped(destination.clone()));
    } else {
        let temporary = tempfile::tempdir().map_err(Error::TemporaryFile)?;
        let client = connect(config)?;
        let (_, status) = client
            .download(
                filename.to_owned(),
                username.to_owned(),
                size,
                temporary.path().display().to_string(),
            )
            .map_err(client_error)?;
        let deadline = std::time::Instant::now() + DOWNLOAD_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(Error::Message("Soulseek download timed out".into()));
            }
            match status.recv_timeout(remaining.min(Duration::from_secs(1))) {
                Ok(DownloadStatus::Completed) => break,
                Ok(DownloadStatus::Failed(reason)) => {
                    return Err(Error::Message(format!(
                        "Soulseek download failed: {}",
                        reason.unwrap_or_else(|| "peer declined the transfer".into())
                    )));
                }
                Ok(DownloadStatus::TimedOut) => {
                    return Err(Error::Message("Soulseek download timed out".into()));
                }
                Ok(DownloadStatus::Cancelled) => {
                    return Err(Error::Message("Soulseek download was cancelled".into()));
                }
                Ok(DownloadStatus::InProgress {
                    bytes_downloaded,
                    total_bytes,
                    ..
                }) => progress(DownloadEvent::Downloading {
                    downloaded: bytes_downloaded,
                    total: Some(total_bytes),
                }),
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Error::Message(
                        "Soulseek download status channel closed".into(),
                    ));
                }
            }
        }
        let source = temporary
            .path()
            .join(filename.rsplit(['/', '\\']).next().unwrap_or(&filename));
        progress(DownloadEvent::Processing);
        ffmpeg.transcode(&source, &destination, &config.audio, &metadata, None)?;
        progress(DownloadEvent::Finished(destination.clone()));
    }
    Ok(DownloadBatchResult {
        plugin_id: "soulseek".into(),
        downloads: vec![DownloadResult {
            plugin_id: "soulseek".into(),
            output_path: destination,
            skipped,
            metadata,
        }],
    })
}

fn connect(config: &Config) -> Result<Client> {
    let auth = config.auth.get("soulseek").ok_or_else(|| {
        Error::InvalidConfig("Soulseek requires [auth.soulseek] username and password".into())
    })?;
    let username = auth
        .values
        .get("username")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidConfig("Soulseek requires auth.soulseek.username".into()))?;
    let password = auth
        .values
        .get("password")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidConfig("Soulseek requires auth.soulseek.password".into()))?;
    let mut client = Client::with_settings(ClientSettings::new(username, password));
    client.connect().map_err(client_error)?;
    if !client.login().map_err(client_error)? {
        return Err(Error::Message("Soulseek login was rejected".into()));
    }
    Ok(client)
}

fn client_error(error: soulseek_rs::SoulseekRs) -> Error {
    Error::Message(format!("Soulseek error: {error}"))
}

fn is_connection_error(error: &Error) -> bool {
    matches!(error, Error::Message(message) if message.starts_with("Soulseek error:"))
}

fn parse_url(value: &str) -> Option<(String, String, u64)> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "soulseek" {
        return None;
    }
    let username = url.host_str()?.to_owned();
    let mut path = None;
    let mut size = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "path" => path = Some(value.into_owned()),
            "size" => size = value.parse().ok(),
            _ => {}
        }
    }
    Some((username, path?, size?))
}

fn parent_folder(path: &str) -> Option<String> {
    let mut components = path.rsplit(['/', '\\']);
    components.next()?;
    components
        .next()
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
}
