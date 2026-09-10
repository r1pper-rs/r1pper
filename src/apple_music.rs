use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
};

use m3u8_rs::{KeyMethod, Playlist};
use mp4_atom::{Decode, Header, Mdat, Moof, ReadFrom};
use reqwest::{
    blocking::Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue, ORIGIN, USER_AGENT},
};
use serde_json::Value;
use tempfile::{Builder, NamedTempFile};
use url::Url;

use crate::{
    OverwritePolicy,
    audio::FfmpegRunner,
    config::Config,
    error::{Error, Result},
    output::{self, OutputTarget},
    pipeline::{DownloadBatchResult, DownloadEvent, DownloadResult},
    plugin::{MediaCandidate, SourceMetadata},
};

const PLUGIN_ID: &str = "apple_music";
const API_BASE: &str = "https://amp-api.music.apple.com/v1/catalog";
const PLAYBACK_URL: &str = "https://play.itunes.apple.com/WebObjects/MZPlay.woa/wa/webPlayback";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131 Safari/537.36";

#[derive(Debug, PartialEq, Eq)]
enum AppleSource {
    Song { storefront: String, id: String },
    Album { storefront: String, id: String },
    Playlist { storefront: String, id: String },
}

pub fn supports(url: &str) -> bool {
    apple_source(url).is_some()
}

pub fn download(
    config: &Config,
    ffmpeg: &FfmpegRunner,
    url: &str,
    overwrite: OverwritePolicy,
    progress: &mut dyn FnMut(DownloadEvent),
) -> Result<DownloadBatchResult> {
    let source = apple_source(url).ok_or_else(|| Error::UnsupportedUrl(url.into()))?;
    progress(DownloadEvent::Resolving);

    let client = apple_client()?;
    let token = developer_token(&client, source.storefront())?;
    let tracks = catalog_tracks(&client, &token, &source)?;
    if tracks.is_empty() {
        return Err(Error::Message(
            "Apple Music collection contains no songs".into(),
        ));
    }

    let settings = config
        .plugin_config
        .get(PLUGIN_ID)
        .and_then(Value::as_object);
    let music_user_token = settings.and_then(|value| setting(value, "music_user_token"));
    let total = tracks.len();
    let overwrite = collection_overwrite_policy(total, overwrite);
    let mut downloads = Vec::with_capacity(total);
    for (index, track) in tracks.iter().enumerate() {
        let metadata = metadata(track);
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
            let media = if let Some(user_token) = music_user_token {
                let playlist = playback_playlist(&client, &token, user_token, track)?;
                let adam_id = track["id"].as_str().ok_or_else(|| {
                    Error::Message("Apple Music track did not contain an ID".into())
                })?;
                download_hls(
                    &client,
                    &playlist,
                    settings.and_then(|value| setting(value, "template_endpoint")),
                    adam_id,
                    progress,
                )?
            } else {
                let preview = preview_url(track).ok_or_else(|| Error::InvalidConfig(
                    "Apple Music requires plugin_config.apple_music.music_user_token for this song; no public preview is available".into(),
                ))?;
                let mut report = |downloaded, total| {
                    progress(DownloadEvent::Downloading { downloaded, total });
                };
                crate::download::fetch(
                    &MediaCandidate {
                        url: preview,
                        headers: BTreeMap::new(),
                        mime_type: Some("audio/mp4".into()),
                        codec: Some("aac".into()),
                    },
                    config.temporary_directory.as_deref(),
                    &Default::default(),
                    Some(&mut report),
                )?
            };
            progress(DownloadEvent::Processing);
            ffmpeg.transcode(media.path(), &destination, &config.audio, &metadata, None)?;
            progress(DownloadEvent::Finished(destination.clone()));
        }
        downloads.push(DownloadResult {
            plugin_id: PLUGIN_ID.into(),
            output_path: destination,
            skipped,
            metadata,
        });
    }
    Ok(DownloadBatchResult {
        plugin_id: PLUGIN_ID.into(),
        downloads,
    })
}

impl AppleSource {
    fn storefront(&self) -> &str {
        match self {
            Self::Song { storefront, .. }
            | Self::Album { storefront, .. }
            | Self::Playlist { storefront, .. } => storefront,
        }
    }

    fn id(&self) -> &str {
        match self {
            Self::Song { id, .. } | Self::Album { id, .. } | Self::Playlist { id, .. } => id,
        }
    }

    fn kind(&self) -> &str {
        match self {
            Self::Song { .. } => "songs",
            Self::Album { .. } => "albums",
            Self::Playlist { .. } => "playlists",
        }
    }
}

fn apple_source(value: &str) -> Option<AppleSource> {
    let url = Url::parse(value).ok()?;
    if url.host_str()? != "music.apple.com" {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    let [storefront, kind, ..] = segments.as_slice() else {
        return None;
    };
    if storefront.len() != 2 || !storefront.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    let id = match *kind {
        "song" | "album" | "playlist" => url
            .query_pairs()
            .find(|(key, _)| key == "i")
            .map(|(_, value)| value.into_owned())
            .or_else(|| segments.last().map(|value| (*value).to_owned()))?,
        _ => return None,
    };
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let storefront = storefront.to_ascii_lowercase();
    Some(match *kind {
        "song" => AppleSource::Song { storefront, id },
        "album" if url.query_pairs().any(|(key, _)| key == "i") => {
            AppleSource::Song { storefront, id }
        }
        "album" => AppleSource::Album { storefront, id },
        "playlist" => AppleSource::Playlist { storefront, id },
        _ => unreachable!(),
    })
}

fn apple_client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(Error::Network)
}

fn developer_token(client: &Client, storefront: &str) -> Result<String> {
    let page = client
        .get(format!("https://music.apple.com/{storefront}/browse"))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()?
        .error_for_status()?
        .text()?;
    if let Some(token) = jwt_in(&page) {
        return Ok(token);
    }
    let page_url = Url::parse(&format!("https://music.apple.com/{storefront}/browse"))
        .expect("Apple Music web player URL is valid");
    for source in script_sources(&page) {
        let Ok(url) = page_url.join(source) else {
            continue;
        };
        let Ok(script) = client
            .get(url)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.text())
        else {
            continue;
        };
        if let Some(token) = jwt_in(&script) {
            return Ok(token);
        }
    }
    Err(Error::Message(
        "could not locate Apple Music developer token in the web player response".into(),
    ))
}

fn catalog_tracks(client: &Client, token: &str, source: &AppleSource) -> Result<Vec<Value>> {
    let endpoint = format!(
        "{API_BASE}/{}/{}/{}",
        source.storefront(),
        source.kind(),
        source.id()
    );
    let resource = api_get(client, token, &endpoint)?;
    let data = resource["data"]
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| {
            Error::Message("Apple Music catalog response did not contain the requested item".into())
        })?;
    if matches!(source, AppleSource::Song { .. }) {
        return Ok(vec![data.clone()]);
    }
    let relationship = data["relationships"]["tracks"]["data"]
        .as_array()
        .ok_or_else(|| {
            Error::Message("Apple Music collection did not include track metadata".into())
        })?;
    Ok(relationship.to_vec())
}

fn api_get(client: &Client, token: &str, endpoint: &str) -> Result<Value> {
    client
        .get(endpoint)
        .headers(api_headers(token)?)
        .send()?
        .error_for_status()?
        .json()
        .map_err(Error::Network)
}

fn playback_playlist(
    client: &Client,
    token: &str,
    user_token: &str,
    track: &Value,
) -> Result<String> {
    let song_id = track["id"]
        .as_str()
        .ok_or_else(|| Error::Message("Apple Music track did not contain an ID".into()))?;
    let response = client
        .post(PLAYBACK_URL)
        .headers(api_headers(token)?)
        .header("Media-User-Token", user_token)
        .json(&serde_json::json!({"salableAdamId": song_id}))
        .send()?
        .error_for_status()?
        .json::<Value>()?;
    find_playlist_url(&response).ok_or_else(|| {
        Error::Message("Apple Music playback response did not contain an HLS playlist URL".into())
    })
}

fn api_headers(token: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|error| {
            Error::Message(format!("invalid Apple Music developer token: {error}"))
        })?,
    );
    headers.insert(ORIGIN, HeaderValue::from_static("https://music.apple.com"));
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    Ok(headers)
}

fn download_hls(
    client: &Client,
    playlist_url: &str,
    template_endpoint: Option<&str>,
    adam_id: &str,
    progress: &mut dyn FnMut(DownloadEvent),
) -> Result<NamedTempFile> {
    let mut playlist_url = Url::parse(playlist_url)
        .map_err(|error| Error::Message(format!("invalid Apple Music HLS URL: {error}")))?;
    let mut body = get_bytes(client, playlist_url.as_str())?;
    let playlist = m3u8_rs::parse_playlist(&body)
        .map_err(|error| {
            Error::Message(format!(
                "could not parse Apple Music HLS playlist: {error:?}"
            ))
        })?
        .1;
    if let Playlist::MasterPlaylist(master) = playlist {
        let variant = master
            .variants
            .iter()
            .filter(|variant| !variant.is_i_frame)
            .max_by_key(|variant| variant.average_bandwidth.unwrap_or(variant.bandwidth))
            .ok_or_else(|| {
                Error::Message("Apple Music HLS master playlist contains no audio variants".into())
            })?;
        playlist_url = playlist_url.join(&variant.uri).map_err(|error| {
            Error::Message(format!("invalid Apple Music HLS variant URL: {error}"))
        })?;
        body = get_bytes(client, playlist_url.as_str())?;
    }
    let media = match m3u8_rs::parse_playlist(&body)
        .map_err(|error| {
            Error::Message(format!(
                "could not parse Apple Music media playlist: {error:?}"
            ))
        })?
        .1
    {
        Playlist::MediaPlaylist(media) => media,
        Playlist::MasterPlaylist(_) => {
            return Err(Error::Message(
                "Apple Music HLS playlist nested more than one level".into(),
            ));
        }
    };
    if media.segments.iter().any(|segment| {
        segment
            .key
            .as_ref()
            .is_some_and(|key| !matches!(key.method, KeyMethod::None | KeyMethod::SampleAES))
    }) {
        return Err(Error::Message(
            "Apple Music HLS encryption is not FairPlay SAMPLE-AES".into(),
        ));
    }
    let encrypted = media.segments.iter().any(|segment| {
        segment
            .key
            .as_ref()
            .is_some_and(|key| key.method == KeyMethod::SampleAES)
    });
    let endpoint = encrypted
        .then(|| {
            template_endpoint.ok_or_else(|| {
                Error::InvalidConfig(
                "encrypted Apple Music HLS requires plugin_config.apple_music.template_endpoint"
                    .into(),
            )
            })
        })
        .transpose()?;
    let mut output = Builder::new()
        .prefix("r1pper-apple-music-")
        .suffix(".mp4")
        .tempfile()
        .map_err(Error::TemporaryFile)?;
    let mut map_url = None;
    let mut templates: BTreeMap<String, temari::rounds::Template> = BTreeMap::new();
    for segment in &media.segments {
        if let Some(map) = &segment.map {
            let url = playlist_url.join(&map.uri).map_err(|error| {
                Error::Message(format!("invalid Apple Music HLS init URL: {error}"))
            })?;
            if map_url.as_ref() != Some(&url) {
                let init = get_bytes(client, url.as_str())?;
                validate_mp4_fragment(&init)?;
                output.write_all(&init).map_err(Error::TemporaryFile)?;
                map_url = Some(url);
            }
        }
        let url = playlist_url.join(&segment.uri).map_err(|error| {
            Error::Message(format!("invalid Apple Music HLS segment URL: {error}"))
        })?;
        let bytes = get_bytes(client, url.as_str())?;
        let bytes = if let Some(key) = segment
            .key
            .as_ref()
            .filter(|key| key.method == KeyMethod::SampleAES)
        {
            let key_uri = key.uri.as_deref().ok_or_else(|| {
                Error::Message("Apple Music encrypted HLS key did not contain a URI".into())
            })?;
            let template = if let Some(template) = templates.get(key_uri) {
                template.clone()
            } else {
                let template = template_for_key(
                    client,
                    endpoint.ok_or_else(|| {
                        Error::InvalidConfig(
                            "encrypted Apple Music HLS requires plugin_config.apple_music.template_endpoint"
                                .into(),
                        )
                    })?,
                    adam_id,
                    key_uri,
                )?;
                templates.insert(key_uri.to_owned(), template.clone());
                template
            };
            decrypt_fragment(&bytes, &template)?
        } else {
            bytes
        };
        output.write_all(&bytes).map_err(Error::TemporaryFile)?;
        progress(DownloadEvent::Downloading {
            downloaded: bytes.len() as u64,
            total: None,
        });
    }
    Ok(output)
}

fn template_for_key(
    client: &Client,
    endpoint: &str,
    adam_id: &str,
    key_uri: &str,
) -> Result<temari::rounds::Template> {
    let endpoint = template_url(endpoint, adam_id, key_uri)?;
    let template = get_bytes(client, endpoint.as_str())?;
    let template = std::str::from_utf8(&template).map_err(|error| {
        Error::Message(format!(
            "Apple Music key endpoint returned invalid UTF-8: {error}"
        ))
    })?;
    let template: Value = serde_json::from_str(template).map_err(|error| {
        Error::Message(format!(
            "Apple Music key endpoint returned invalid JSON: {error}"
        ))
    })?;
    if let Some(code) = template.get("code").and_then(Value::as_i64) {
        if code != 0 {
            let message = template
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(Error::Message(format!(
                "Apple Music key endpoint returned code {code}: {message}"
            )));
        }
    }
    let template = template.get("data").unwrap_or(&template).to_string();
    temari::template::template_from_json(&template).map_err(|error| {
        Error::Message(format!(
            "could not load Apple Music decryption template: {error}"
        ))
    })
}

fn template_url(endpoint: &str, adam_id: &str, key_uri: &str) -> Result<Url> {
    let mut endpoint = Url::parse(endpoint).map_err(|error| {
        Error::InvalidConfig(format!("invalid Apple Music template_endpoint: {error}"))
    })?;
    endpoint
        .query_pairs_mut()
        .append_pair("adamId", adam_id)
        .append_pair("uri", key_uri);
    Ok(endpoint)
}

fn decrypt_fragment(fragment: &[u8], template: &temari::rounds::Template) -> Result<Vec<u8>> {
    let atoms = fragment_atoms(fragment)?;
    let moofs: Vec<_> = atoms.iter().filter(|atom| atom.kind == *b"moof").collect();
    let mdats: Vec<_> = atoms.iter().filter(|atom| atom.kind == *b"mdat").collect();
    let [moof] = moofs.as_slice() else {
        return Err(Error::Message(
            "encrypted Apple Music fragment must contain exactly one moof box".into(),
        ));
    };
    let [mdat] = mdats.as_slice() else {
        return Err(Error::Message(
            "encrypted Apple Music fragment must contain exactly one mdat box".into(),
        ));
    };
    let moof_data = &fragment[moof.start..moof.end];
    let parsed_moof = Moof::decode(&mut Cursor::new(moof_data)).map_err(|error| {
        Error::Message(format!(
            "could not parse Apple Music fragment moof: {error}"
        ))
    })?;
    Mdat::decode(&mut Cursor::new(&fragment[mdat.start..mdat.end])).map_err(|error| {
        Error::Message(format!(
            "could not parse Apple Music fragment mdat: {error}"
        ))
    })?;
    let samples = fragment_samples(&parsed_moof, moof.start, mdat)?;
    if samples.is_empty() {
        return Err(Error::Message(
            "encrypted Apple Music fragment has no mapped samples to decrypt".into(),
        ));
    }

    let ciphertext: Vec<_> = samples
        .iter()
        .map(|sample| &fragment[sample.start..sample.end])
        .collect();
    let plaintext = temari::rounds::decrypt_par(template, &ciphertext);
    let mut decrypted = fragment.to_vec();
    for (sample, plaintext) in samples.iter().zip(plaintext) {
        decrypted[sample.start..sample.end].copy_from_slice(&plaintext);
    }
    // Retain box sizes so trun data offsets remain valid while removing per-fragment DRM data.
    strip_fragment_encryption_metadata(&mut decrypted, moof.start, moof.end, moof.header_len)?;
    Ok(decrypted)
}

#[derive(Clone, Copy)]
struct FragmentAtom {
    kind: [u8; 4],
    start: usize,
    end: usize,
    header_len: usize,
}

fn fragment_atoms(bytes: &[u8]) -> Result<Vec<FragmentAtom>> {
    let mut offset = 0;
    let mut atoms = Vec::new();
    while offset < bytes.len() {
        let header = Header::read_from(&mut Cursor::new(&bytes[offset..])).map_err(|error| {
            Error::Message(format!("invalid Apple Music fragmented MP4 box: {error}"))
        })?;
        let header_len = if bytes.get(offset..offset + 4) == Some(&[0, 0, 0, 1]) {
            16
        } else {
            8
        };
        let size = header.size.unwrap_or(bytes.len() - offset - header_len);
        let end = offset
            .checked_add(header_len)
            .and_then(|value| value.checked_add(size))
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                Error::Message("Apple Music fragmented MP4 box exceeds segment".into())
            })?;
        atoms.push(FragmentAtom {
            kind: bytes[offset + 4..offset + 8]
                .try_into()
                .expect("MP4 header has type"),
            start: offset,
            end,
            header_len,
        });
        offset = end;
    }
    Ok(atoms)
}

fn fragment_samples(
    moof: &Moof,
    moof_start: usize,
    mdat: &FragmentAtom,
) -> Result<Vec<FragmentAtom>> {
    let mut samples = Vec::new();
    for traf in &moof.traf {
        if traf.tfhd.base_data_offset.is_some() {
            return Err(Error::Message(
                "Apple Music fragment uses unsupported absolute sample offsets".into(),
            ));
        }
        for trun in &traf.trun {
            let data_offset = trun.data_offset.ok_or_else(|| {
                Error::Message("Apple Music fragment omits a trun sample data offset".into())
            })?;
            let mut start = moof_start
                .checked_add_signed(data_offset as isize)
                .ok_or_else(|| {
                    Error::Message("Apple Music sample offset overflows fragment".into())
                })?;
            for entry in &trun.entries {
                let size = entry
                    .size
                    .or(traf.tfhd.default_sample_size)
                    .ok_or_else(|| {
                        Error::Message("Apple Music fragment omits a sample size".into())
                    })? as usize;
                let end = start.checked_add(size).ok_or_else(|| {
                    Error::Message("Apple Music sample size overflows fragment".into())
                })?;
                if start < mdat.start + mdat.header_len || end > mdat.end {
                    return Err(Error::Message(
                        "Apple Music fragment sample mapping is outside mdat".into(),
                    ));
                }
                samples.push(FragmentAtom {
                    kind: *b"samp",
                    start,
                    end,
                    header_len: 0,
                });
                start = end;
            }
        }
    }
    samples.sort_by_key(|sample| sample.start);
    if samples.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(Error::Message(
            "Apple Music fragment has overlapping sample mappings".into(),
        ));
    }
    Ok(samples)
}

fn strip_fragment_encryption_metadata(
    bytes: &mut [u8],
    start: usize,
    end: usize,
    header_len: usize,
) -> Result<()> {
    let mut offset = start + header_len;
    while offset < end {
        let atoms = fragment_atoms(&bytes[offset..end])?;
        let atom = atoms.first().ok_or_else(|| {
            Error::Message("Apple Music fragment has malformed nested encryption metadata".into())
        })?;
        let atom_start = offset + atom.start;
        let atom_end = offset + atom.end;
        if atom.kind == *b"traf" {
            strip_fragment_encryption_metadata(bytes, atom_start, atom_end, atom.header_len)?;
        } else if matches!(&atom.kind, b"senc" | b"saiz" | b"saio") {
            bytes[atom_start + 4..atom_start + 8].copy_from_slice(b"free");
        }
        offset = atom_end;
    }
    Ok(())
}

fn validate_mp4_fragment(bytes: &[u8]) -> Result<()> {
    Header::read_from(&mut Cursor::new(bytes))
        .map(|_| ())
        .map_err(|error| {
            Error::Message(format!(
                "invalid Apple Music fragmented MP4 init segment: {error}"
            ))
        })
}

fn get_bytes(client: &Client, url: &str) -> Result<Vec<u8>> {
    client
        .get(url)
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()?
        .error_for_status()?
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(Error::Network)
}

fn metadata(track: &Value) -> SourceMetadata {
    let attributes = &track["attributes"];
    SourceMetadata {
        title: string(attributes, "name"),
        artist: string(attributes, "artistName"),
        album: string(attributes, "albumName"),
        album_artist: string(attributes, "artistName"),
        track_number: number(attributes, "trackNumber"),
        disc_number: number(attributes, "discNumber"),
        date: string(attributes, "releaseDate"),
        genre: attributes["genreNames"]
            .as_array()
            .and_then(|genres| genres.first())
            .and_then(Value::as_str)
            .map(str::to_owned),
        artwork_url: artwork_url(attributes),
    }
}

fn preview_url(track: &Value) -> Option<String> {
    track["attributes"]["previews"]
        .as_array()?
        .first()?
        .get("url")?
        .as_str()
        .map(str::to_owned)
}

fn artwork_url(attributes: &Value) -> Option<String> {
    let artwork = &attributes["artwork"];
    let url = artwork["url"].as_str()?;
    Some(
        url.replace("{w}", "1200")
            .replace("{h}", "1200")
            .replace("{f}", "jpg"),
    )
}

fn string(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

fn number(value: &Value, key: &str) -> Option<u32> {
    value[key].as_u64().and_then(|value| value.try_into().ok())
}

fn setting<'a>(settings: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    settings
        .get(key)?
        .as_str()
        .filter(|value| !value.is_empty())
}

fn jwt_in(value: &str) -> Option<String> {
    value.split(|character: char| !matches!(character, 'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.'))
        .find(|candidate| candidate.starts_with("eyJ") && candidate.matches('.').count() == 2)
        .map(str::to_owned)
}

fn script_sources(page: &str) -> impl Iterator<Item = &str> {
    page.split("src=").filter_map(|tail| {
        let quote = tail.chars().next()?;
        matches!(quote, '\'' | '"').then_some(())?;
        let source = &tail[quote.len_utf8()..];
        let end = source.find(quote)?;
        let source = &source[..end];
        source.ends_with(".js").then_some(source)
    })
}

fn find_playlist_url(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if value.contains(".m3u8") => Some(value.to_owned()),
        Value::Array(values) => values.iter().find_map(find_playlist_url),
        Value::Object(values) => values.values().find_map(find_playlist_url),
        _ => None,
    }
}

fn collection_overwrite_policy(track_count: usize, requested: OverwritePolicy) -> OverwritePolicy {
    if track_count > 1 && !matches!(requested, OverwritePolicy::Overwrite) {
        OverwritePolicy::Skip
    } else {
        requested
    }
}
