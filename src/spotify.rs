use std::{collections::BTreeMap, io::Write, path::Path};

use librespot::{
    core::{SpotifyUri, authentication::Credentials, session::Session},
    metadata::Metadata,
    playback::{
        audio_backend::{Sink, SinkError},
        config::PlayerConfig,
        convert::Converter,
        decoder::AudioPacket,
        mixer::NoOpVolume,
        player::Player,
    },
};
use librespot_oauth::{DeviceAuthClient, DeviceAuthClientBuilder, DeviceAuthorization};
use tempfile::{Builder, NamedTempFile};

use crate::{
    audio::FfmpegRunner,
    auth::AuthCredentials,
    config::Config,
    error::{Error, Result},
    output::{self, OutputTarget},
    pipeline::{DownloadBatchResult, DownloadEvent, DownloadResult},
    plugin::{MediaCandidate, SourceMetadata},
    search::SearchResult,
};

const CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const SCOPES: &[&str] = &["streaming"];

/// A pending Spotify device authorization that can be displayed by a UI.
pub struct DeviceLogin {
    client: DeviceAuthClient,
    authorization: DeviceAuthorization,
    pub user_code: String,
    pub verification_url: String,
}

impl std::fmt::Debug for DeviceLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceLogin")
            .field("user_code", &self.user_code)
            .field("verification_url", &self.verification_url)
            .finish_non_exhaustive()
    }
}

pub fn supports(url: &str) -> bool {
    spotify_source(url).is_some()
}

pub fn search(config: &Config, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let access_token = config
        .auth
        .get("spotify")
        .and_then(|auth| auth.values.get("access_token"))
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            Error::InvalidConfig("Spotify requires `r1pper auth login spotify`".into())
        })?;
    #[derive(serde::Deserialize)]
    struct Response {
        tracks: Tracks,
    }
    #[derive(serde::Deserialize)]
    struct Tracks {
        items: Vec<Track>,
    }
    #[derive(serde::Deserialize)]
    struct Track {
        id: String,
        name: String,
        artists: Vec<Artist>,
        album: Album,
    }
    #[derive(serde::Deserialize)]
    struct Artist {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct Album {
        name: String,
        #[serde(default)]
        images: Vec<Image>,
    }
    #[derive(serde::Deserialize)]
    struct Image {
        url: String,
    }
    let request_url = format!(
        "https://api.spotify.com/v1/search?{}",
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("q", query)
            .append_pair("type", "track")
            .append_pair("limit", &limit.min(50).max(1).to_string())
            .finish()
    );
    let response: Response = reqwest::blocking::Client::new()
        .get(request_url)
        .bearer_auth(access_token)
        .send()?
        .error_for_status()?
        .json()?;
    Ok(response
        .tracks
        .items
        .into_iter()
        .map(|track| SearchResult {
            plugin_id: "spotify".into(),
            title: track.name,
            url: format!("spotify:track:{}", track.id),
            artist: (!track.artists.is_empty()).then(|| {
                track
                    .artists
                    .into_iter()
                    .map(|artist| artist.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
            album: Some(track.album.name),
            size: None,
            artwork_url: track.album.images.into_iter().next().map(|image| image.url),
        })
        .collect())
}

pub fn login(config_path: &Path) -> Result<()> {
    let login = begin_login()?;
    finish_login(login, config_path)
}

/// Requests a Spotify device code without starting the blocking approval poll.
pub fn begin_login() -> Result<DeviceLogin> {
    let client = DeviceAuthClientBuilder::new(CLIENT_ID, SCOPES.to_vec())
        .build()
        .map_err(|error| {
            Error::Message(format!("could not create Spotify device client: {error}"))
        })?;
    let authorization = client
        .request_device_code()
        .map_err(|error| Error::Message(format!("Spotify device authorization failed: {error}")))?;
    Ok(DeviceLogin {
        user_code: authorization.user_code().into(),
        verification_url: authorization.url().into(),
        client,
        authorization,
    })
}

/// Waits for approval of a previously requested Spotify device code and saves its tokens.
pub fn finish_login(login: DeviceLogin, config_path: &Path) -> Result<()> {
    let token = login
        .client
        .poll_for_token(&login.authorization)
        .map_err(|error| Error::Message(format!("Spotify device authorization failed: {error}")))?;
    Config::save_auth_credentials(
        config_path,
        "spotify",
        &AuthCredentials {
            scheme: "spotify_device".into(),
            values: [
                ("access_token".into(), token.access_token),
                ("refresh_token".into(), token.refresh_token),
            ]
            .into_iter()
            .collect(),
        },
    )?;
    println!(
        "Spotify authorization complete. Credentials saved to {}.",
        config_path.display()
    );
    Ok(())
}

pub fn download(
    config: &Config,
    ffmpeg: &FfmpegRunner,
    url: &str,
    overwrite: crate::OverwritePolicy,
    progress: &mut dyn FnMut(DownloadEvent),
) -> Result<DownloadBatchResult> {
    let source = spotify_source(url).ok_or_else(|| Error::UnsupportedUrl(url.into()))?;
    let access_token = config
        .auth
        .get("spotify")
        .filter(|auth| auth.scheme == "spotify_device")
        .and_then(|auth| auth.values.get("access_token"))
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            Error::InvalidConfig("Spotify requires `r1pper auth login spotify`".into())
        })?;
    progress(DownloadEvent::Resolving);
    let track_ids = match source {
        SpotifySource::Track(id) => vec![id.to_owned()],
        SpotifySource::Album(id) => collection_track_ids("album", id, access_token)?,
        SpotifySource::Playlist(id) => collection_track_ids("playlist", id, access_token)?,
    };
    if track_ids.is_empty() {
        return Err(Error::Message(
            "Spotify collection contains no tracks".into(),
        ));
    }
    let total = track_ids.len();
    let overwrite = collection_overwrite_policy(total, overwrite);
    let mut downloads = Vec::with_capacity(total);
    for (index, track_id) in track_ids.iter().enumerate() {
        let metadata = track_metadata(track_id, access_token)?;
        progress(DownloadEvent::StartingItem {
            index: index + 1,
            total,
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
            let (source, _) = download_track(track_id, access_token)?;
            let artwork = if config.audio.embed_artwork {
                metadata
                    .artwork_url
                    .as_ref()
                    .map(|url| {
                        crate::download::fetch(
                            &MediaCandidate {
                                url: url.clone(),
                                headers: BTreeMap::new(),
                                mime_type: None,
                                codec: None,
                            },
                            config.temporary_directory.as_deref(),
                            &Default::default(),
                            None,
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            progress(DownloadEvent::Processing);
            ffmpeg.transcode(
                source.path(),
                &destination,
                &config.audio,
                &metadata,
                artwork.as_ref().map(|file| file.path()),
            )?;
            progress(DownloadEvent::Finished(destination.clone()));
        }
        downloads.push(DownloadResult {
            plugin_id: "spotify".into(),
            output_path: destination,
            skipped,
            metadata,
        });
    }
    Ok(DownloadBatchResult {
        plugin_id: "spotify".into(),
        downloads,
    })
}

fn track_metadata(track_id: &str, access_token: &str) -> Result<SourceMetadata> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| Error::Message(error.to_string()))?;
    runtime.block_on(async move {
        let session = Session::new(Default::default(), None);
        session
            .connect(Credentials::with_access_token(access_token), true)
            .await
            .map_err(|error| Error::Message(format!("could not connect to Spotify: {error}")))?;
        let uri = SpotifyUri::from_uri(&format!("spotify:track:{track_id}"))
            .map_err(|error| Error::Message(format!("invalid Spotify track ID: {error}")))?;
        source_metadata(&session, &uri).await
    })
}

fn collection_track_ids(kind: &str, id: &str, access_token: &str) -> Result<Vec<String>> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| Error::Message(error.to_string()))?;
    runtime.block_on(async move {
        let session = Session::new(Default::default(), None);
        session
            .connect(Credentials::with_access_token(access_token), true)
            .await
            .map_err(|error| Error::Message(format!("could not connect to Spotify: {error}")))?;
        let uri = SpotifyUri::from_uri(&format!("spotify:{kind}:{id}"))
            .map_err(|error| Error::Message(format!("invalid Spotify {kind} ID: {error}")))?;
        let tracks = match kind {
            "album" => librespot::metadata::Album::get(&session, &uri)
                .await
                .map_err(|error| Error::Message(format!("could not load Spotify album: {error}")))?
                .tracks()
                .map(SpotifyUri::to_id)
                .collect(),
            "playlist" => librespot::metadata::Playlist::get(&session, &uri)
                .await
                .map_err(|error| {
                    Error::Message(format!("could not load Spotify playlist: {error}"))
                })?
                .tracks()
                .filter(|uri| matches!(uri, SpotifyUri::Track { .. }))
                .map(SpotifyUri::to_id)
                .collect(),
            _ => unreachable!("only Spotify album and playlist collections are supported"),
        };
        Ok(tracks)
    })
}

fn download_track(track_id: &str, access_token: &str) -> Result<(NamedTempFile, SourceMetadata)> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| Error::Message(error.to_string()))?;
    runtime.block_on(async move {
        let session = Session::new(Default::default(), None);
        session
            .connect(Credentials::with_access_token(access_token), true)
            .await
            .map_err(|error| Error::Message(format!("could not connect to Spotify: {error}")))?;
        let uri = SpotifyUri::from_uri(&format!("spotify:track:{track_id}"))
            .map_err(|error| Error::Message(format!("invalid Spotify track ID: {error}")))?;
        let metadata = source_metadata(&session, &uri).await?;
        let (sink, mut receiver) = SampleSink::new();
        let player = Player::new(
            PlayerConfig::default(),
            session,
            Box::new(NoOpVolume),
            move || Box::new(sink),
        );
        player.load(uri, true, 0);
        tokio::spawn(async move {
            player.await_end_of_track().await;
            player.stop();
        });
        let mut samples = Vec::new();
        while let Some(event) = receiver.recv().await {
            match event {
                SampleEvent::Samples(mut values) => samples.append(&mut values),
                SampleEvent::Finished => break,
            }
        }
        wav_file(&samples).map(|file| (file, metadata))
    })
}

async fn source_metadata(session: &Session, uri: &SpotifyUri) -> Result<SourceMetadata> {
    let track = librespot::metadata::Track::get(session, uri)
        .await
        .map_err(|error| {
            Error::Message(format!("could not load Spotify track metadata: {error}"))
        })?;
    let album = librespot::metadata::Album::get(session, &track.album.id)
        .await
        .map_err(|error| {
            Error::Message(format!("could not load Spotify album metadata: {error}"))
        })?;
    let artist = (!track.artists.0.is_empty()).then(|| {
        track
            .artists
            .0
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    });
    Ok(SourceMetadata {
        title: Some(track.name),
        artist,
        album: Some(album.name),
        album_artist: None,
        track_number: Some(track.number as u32),
        disc_number: Some(track.disc_number as u32),
        date: None,
        genre: None,
        artwork_url: track
            .album
            .covers
            .0
            .first()
            .map(|image| format!("https://i.scdn.co/image/{}", image.id)),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum SpotifySource<'a> {
    Track(&'a str),
    Album(&'a str),
    Playlist(&'a str),
}

fn spotify_source(value: &str) -> Option<SpotifySource<'_>> {
    for kind in ["track", "album", "playlist"] {
        if let Some(id) = value.strip_prefix(&format!("spotify:{kind}:")) {
            return valid_id(id).then(|| source_from_id(kind, id));
        }
        if let Some((_, path)) = value.split_once(&format!("open.spotify.com/{kind}/")) {
            let id = path.split(['/', '?', '#']).next()?;
            return valid_id(id).then(|| source_from_id(kind, id));
        }
    }
    None
}

fn source_from_id<'a>(kind: &str, id: &'a str) -> SpotifySource<'a> {
    match kind {
        "track" => SpotifySource::Track(id),
        "album" => SpotifySource::Album(id),
        "playlist" => SpotifySource::Playlist(id),
        _ => unreachable!("only supported Spotify resource types are parsed"),
    }
}

fn valid_id(value: &str) -> bool {
    value.len() == 22 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn collection_overwrite_policy(
    track_count: usize,
    requested: crate::OverwritePolicy,
) -> crate::OverwritePolicy {
    if track_count > 1 && !matches!(requested, crate::OverwritePolicy::Overwrite) {
        crate::OverwritePolicy::Skip
    } else {
        requested
    }
}

fn wav_file(samples: &[i32]) -> Result<NamedTempFile> {
    let mut file = Builder::new()
        .prefix("r1pper-spotify-")
        .suffix(".wav")
        .tempfile()
        .map_err(Error::TemporaryFile)?;
    let data_len = u32::try_from(samples.len().saturating_mul(2))
        .map_err(|_| Error::Message("Spotify track is too large".into()))?;
    file.write_all(b"RIFF").map_err(Error::TemporaryFile)?;
    file.write_all(&(36u32 + data_len).to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    file.write_all(b"WAVEfmt ").map_err(Error::TemporaryFile)?;
    file.write_all(&16u32.to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    file.write_all(&1u16.to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    file.write_all(&2u16.to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    file.write_all(&44_100u32.to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    file.write_all(&176_400u32.to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    file.write_all(&4u16.to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    file.write_all(&16u16.to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    file.write_all(b"data").map_err(Error::TemporaryFile)?;
    file.write_all(&data_len.to_le_bytes())
        .map_err(Error::TemporaryFile)?;
    for sample in samples {
        file.write_all(&(*sample as i16).to_le_bytes())
            .map_err(Error::TemporaryFile)?;
    }
    Ok(file)
}

enum SampleEvent {
    Samples(Vec<i32>),
    Finished,
}

struct SampleSink {
    sender: tokio::sync::mpsc::UnboundedSender<SampleEvent>,
}

impl SampleSink {
    fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<SampleEvent>) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }
}

impl Sink for SampleSink {
    fn start(&mut self) -> std::result::Result<(), SinkError> {
        Ok(())
    }
    fn stop(&mut self) -> std::result::Result<(), SinkError> {
        self.sender
            .send(SampleEvent::Finished)
            .map_err(|_| SinkError::OnWrite("Spotify sample receiver closed".into()))
    }
    fn write(
        &mut self,
        packet: AudioPacket,
        converter: &mut Converter,
    ) -> std::result::Result<(), SinkError> {
        let samples = converter.f64_to_s16(
            packet
                .samples()
                .map_err(|error| SinkError::OnWrite(error.to_string()))?,
        );
        self.sender
            .send(SampleEvent::Samples(
                samples.into_iter().map(i32::from).collect(),
            ))
            .map_err(|_| SinkError::OnWrite("Spotify sample receiver closed".into()))
    }
}
