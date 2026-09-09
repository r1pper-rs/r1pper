#![no_main]

use std::collections::BTreeMap;

use anyhow::{Context, anyhow};
use extism_pdk::*;
use serde::{Deserialize, Deserializer, Serialize};

const SIGN_SALT: &str = "XGRlBW9FXlekgbPrRHuSiA";
const BASE_URL: &str = "https://api.music.yandex.net";

#[derive(Deserialize)]
struct ResolveRequest {
    api_version: u32,
    url: String,
    http: HttpSettings,
    auth: Option<AuthCredentials>,
    plugin_config: Option<PluginConfig>,
}

#[derive(Default, Deserialize)]
struct PluginConfig {
    artist_max_tracks: Option<usize>,
}

#[derive(Default, Deserialize)]
struct HttpSettings {
    user_agent: Option<String>,
    headers: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct AuthCredentials {
    scheme: String,
    #[serde(flatten)]
    values: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct DownloadInfoResponse {
    result: Vec<DownloadInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadInfo {
    codec: Option<String>,
    download_info_url: Option<String>,
}

#[derive(Deserialize)]
struct FullInfoResponse {
    result: FullInfo,
}

#[derive(Deserialize)]
struct FullInfo {
    track: Track,
}

#[derive(Deserialize)]
struct AlbumWithTracksResponse {
    result: AlbumWithTracks,
}

#[derive(Deserialize)]
struct AlbumWithTracks {
    #[serde(default)]
    volumes: Vec<Vec<Track>>,
}

#[derive(Deserialize)]
struct ArtistTracksResponse {
    result: ArtistTracks,
}

#[derive(Deserialize)]
struct ArtistTracks {
    #[serde(default)]
    tracks: Vec<Track>,
}

#[derive(Deserialize)]
struct SearchApiResponse {
    result: SearchApiResult,
}

#[derive(Default, Deserialize)]
struct SearchApiResult {
    tracks: Option<SearchTracks>,
}

#[derive(Default, Deserialize)]
struct SearchTracks {
    #[serde(default)]
    results: Vec<SearchTrack>,
}

#[derive(Deserialize)]
struct SearchTrack {
    #[serde(deserialize_with = "deserialize_id")]
    id: Option<String>,
    title: Option<String>,
    #[serde(default)]
    artists: Vec<Artist>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Track {
    #[serde(deserialize_with = "deserialize_id")]
    id: Option<String>,
    title: Option<String>,
    version: Option<String>,
    #[serde(default)]
    artists: Vec<Artist>,
    #[serde(default)]
    albums: Vec<Album>,
    cover_uri: Option<String>,
    genre: Option<String>,
}

fn deserialize_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value)),
        Some(serde_json::Value::Number(value)) => Ok(Some(value.to_string())),
        Some(_) => Err(serde::de::Error::custom(
            "track ID must be a string or number",
        )),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Album {
    title: Option<String>,
    #[serde(default)]
    artists: Vec<Artist>,
    cover_uri: Option<String>,
    release_date: Option<String>,
    year: Option<u32>,
    track_position: Option<TrackPosition>,
}

#[derive(Deserialize)]
struct Artist {
    name: Option<String>,
}

#[derive(Deserialize)]
struct TrackPosition {
    index: Option<u32>,
    volume: Option<u32>,
}

#[derive(Serialize)]
struct ResolveResponse {
    api_version: u32,
    #[serde(default)]
    candidates: Vec<MediaCandidate>,
    #[serde(default)]
    metadata: SourceMetadata,
    #[serde(default)]
    items: Vec<ResolvedItem>,
    #[serde(default)]
    deferred_urls: Vec<String>,
}

#[derive(Serialize)]
struct ResolvedItem {
    candidates: Vec<MediaCandidate>,
    metadata: SourceMetadata,
}

#[derive(Serialize)]
struct MediaCandidate {
    url: String,
    headers: BTreeMap<String, String>,
    mime_type: String,
    codec: String,
}

#[derive(Default, Serialize)]
struct SourceMetadata {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    track_number: Option<u32>,
    disc_number: Option<u32>,
    date: Option<String>,
    genre: Option<String>,
    artwork_url: Option<String>,
}

#[derive(Deserialize)]
struct SearchRequest {
    api_version: u32,
    query: String,
    limit: usize,
    http: HttpSettings,
    auth: Option<AuthCredentials>,
}

#[derive(Serialize)]
struct SearchResponse {
    api_version: u32,
    results: Vec<SearchResult>,
}

#[derive(Serialize)]
struct SearchResult {
    title: String,
    url: String,
    artist: Option<String>,
    album: Option<String>,
    size: Option<u64>,
}

/// Resolve a Yandex Music track URL to its signed MP3 download URL.
#[plugin_fn]
pub fn resolve(Json(input): Json<ResolveRequest>) -> FnResult<Json<ResolveResponse>> {
    if input.api_version != 1 {
        return Err(anyhow!("unsupported plugin API version {}", input.api_version).into());
    }
    let artist_id = extract_artist_id(&input.url);
    let album_id = extract_album_id(&input.url);
    let track_id = if artist_id.is_none() && album_id.is_none() {
        Some(extract_track_id(&input.url)?)
    } else {
        None
    };
    let token = input
        .auth
        .as_ref()
        .filter(|auth| auth.scheme == "oauth" || auth.scheme == "bearer")
        .and_then(|auth| auth.values.get("access_token"))
        .filter(|token| !token.is_empty())
        .context("Yandex Music requires [auth.yandex_music] scheme = \"oauth\" and access_token")?;
    if let Some(album_id) = album_id {
        return resolve_album(album_id, token, &input.http);
    }
    if let Some(artist_id) = artist_id {
        let max_tracks = input
            .plugin_config
            .as_ref()
            .and_then(|config| config.artist_max_tracks)
            .unwrap_or(10)
            .clamp(1, 1_000);
        return resolve_artist(artist_id, token, &input.http, max_tracks);
    }
    let track_id = track_id.expect("track ID is present when the URL is not an album");
    let item = resolve_track(track_id, token, &input.http)?;

    Ok(Json(ResolveResponse {
        api_version: 1,
        candidates: item.candidates,
        metadata: item.metadata,
        items: Vec::new(),
        deferred_urls: Vec::new(),
    }))
}

/// Search Yandex Music tracks and return URLs that r1pper can download.
#[plugin_fn]
pub fn search(Json(input): Json<SearchRequest>) -> FnResult<Json<SearchResponse>> {
    if input.api_version != 1 {
        return Err(anyhow!("unsupported plugin API version {}", input.api_version).into());
    }
    let token = input
        .auth
        .as_ref()
        .filter(|auth| auth.scheme == "oauth" || auth.scheme == "bearer")
        .and_then(|auth| auth.values.get("access_token"))
        .filter(|token| !token.is_empty())
        .context("Yandex Music requires [auth.yandex_music] scheme = \"oauth\" and access_token")?;
    let mut headers = input.http.headers;
    headers.insert("Authorization".into(), format!("OAuth {token}"));
    headers.insert("Accept".into(), "application/json".into());
    let query = percent_encode(&input.query);
    let response = get(
        &format!("{BASE_URL}/search?text={query}&type=track&page=0&nocorrect=false"),
        headers,
        input.http.user_agent.as_deref(),
    )?;
    if response.status_code() != 200 {
        return Err(anyhow!(
            "Yandex Music search returned HTTP {}",
            response.status_code()
        )
        .into());
    }
    let response: SearchApiResponse = response.json().context("could not parse search JSON")?;
    let results = response
        .result
        .tracks
        .unwrap_or_default()
        .results
        .into_iter()
        .filter_map(|track| {
            let id = track.id?;
            let title = track
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "Unknown track".into());
            let artist = artist_names(&track.artists)
                .filter(|artist| !artist.trim().is_empty());
            Some(SearchResult {
                title,
                url: format!("https://music.yandex.ru/track/{id}"),
                artist,
                album: None,
                size: None,
            })
        })
        .take(input.limit.max(1))
        .collect();
    Ok(Json(SearchResponse {
        api_version: 1,
        results,
    }))
}

fn resolve_track(track_id: &str, token: &str, http: &HttpSettings) -> FnResult<ResolvedItem> {
    let mut headers = http.headers.clone();
    headers.insert("Authorization".into(), format!("OAuth {token}"));
    headers.insert("Accept".into(), "application/json".into());
    let full_info = get(
        &format!("{BASE_URL}/tracks/{track_id}/full-info"),
        headers,
        http.user_agent.as_deref(),
    )?;
    if full_info.status_code() != 200 {
        return Err(anyhow!(
            "Yandex Music full-info returned HTTP {}",
            full_info.status_code()
        )
        .into());
    }
    let full_info: FullInfoResponse = full_info.json().context("could not parse full-info JSON")?;
    Ok(ResolvedItem {
        candidates: vec![signed_candidate(track_id, token, http)?],
        metadata: source_metadata(full_info.result.track),
    })
}

fn signed_candidate(track_id: &str, token: &str, http: &HttpSettings) -> FnResult<MediaCandidate> {
    let info_url = format!("{BASE_URL}/tracks/{track_id}/download-info");
    let mut info_headers = http.headers.clone();
    info_headers.insert("Authorization".into(), format!("OAuth {token}"));
    info_headers.insert("Accept".into(), "application/json".into());
    let info = get(&info_url, info_headers, http.user_agent.as_deref())?;
    if matches!(info.status_code(), 401 | 403) {
        return Err(anyhow!("Yandex Music rejected the OAuth token").into());
    }
    if info.status_code() != 200 {
        return Err(anyhow!(
            "Yandex Music download-info returned HTTP {}",
            info.status_code()
        )
        .into());
    }
    let info: DownloadInfoResponse = info.json().context("could not parse download-info JSON")?;
    let download_info_url = info
        .result
        .iter()
        .find(|item| item.codec.as_deref() == Some("mp3"))
        .or_else(|| info.result.first())
        .and_then(|item| item.download_info_url.as_deref())
        .context("download-info did not contain a downloadInfoUrl")?;

    let mut xml_headers = http.headers.clone();
    xml_headers.insert("Accept".into(), "application/xml".into());
    let xml = get(download_info_url, xml_headers, http.user_agent.as_deref())?;
    if xml.status_code() != 200 {
        return Err(anyhow!(
            "Yandex Music download XML returned HTTP {}",
            xml.status_code()
        )
        .into());
    }
    let xml = String::from_utf8(xml.body()).context("download XML was not UTF-8")?;
    let host = xml_tag(&xml, "host").context("download XML did not contain host")?;
    let path = xml_tag(&xml, "path").context("download XML did not contain path")?;
    let timestamp = xml_tag(&xml, "ts").context("download XML did not contain ts")?;
    let secret = xml_tag(&xml, "s").context("download XML did not contain s")?;
    let path_no_slash = path.strip_prefix('/').unwrap_or(&path);
    let signature = format!(
        "{:x}",
        md5::compute(format!("{SIGN_SALT}{path_no_slash}{secret}"))
    );
    let direct_url = format!("https://{host}/get-mp3/{signature}/{timestamp}{path}");

    Ok(MediaCandidate {
        url: direct_url,
        headers: BTreeMap::new(),
        mime_type: "audio/mpeg".into(),
        codec: "mp3".into(),
    })
}

fn resolve_album(
    album_id: &str,
    token: &str,
    http: &HttpSettings,
) -> FnResult<Json<ResolveResponse>> {
    let mut headers = http.headers.clone();
    headers.insert("Authorization".into(), format!("OAuth {token}"));
    headers.insert("Accept".into(), "application/json".into());
    let response = get(
        &format!("{BASE_URL}/albums/{album_id}/with-tracks"),
        headers,
        http.user_agent.as_deref(),
    )?;
    if response.status_code() != 200 {
        return Err(anyhow!(
            "Yandex Music album with-tracks returned HTTP {}",
            response.status_code()
        )
        .into());
    }
    let album: AlbumWithTracksResponse = response
        .json()
        .context("could not parse album with-tracks JSON")?;
    let mut deferred_urls = Vec::new();
    for track in album.result.volumes.into_iter().flatten() {
        let track_id = track
            .id
            .as_deref()
            .context("album response contained a track without an ID")?;
        deferred_urls.push(format!("https://music.yandex.ru/track/{track_id}"));
    }
    if deferred_urls.is_empty() {
        return Err(anyhow!("album response did not contain downloadable tracks").into());
    }
    Ok(Json(ResolveResponse {
        api_version: 1,
        candidates: Vec::new(),
        metadata: SourceMetadata::default(),
        items: Vec::new(),
        deferred_urls,
    }))
}

fn resolve_artist(
    artist_id: &str,
    token: &str,
    http: &HttpSettings,
    max_tracks: usize,
) -> FnResult<Json<ResolveResponse>> {
    const PAGE_SIZE: usize = 100;
    let mut page = 0;
    let mut deferred_urls = Vec::new();
    while deferred_urls.len() < max_tracks {
        let mut headers = http.headers.clone();
        headers.insert("Authorization".into(), format!("OAuth {token}"));
        headers.insert("Accept".into(), "application/json".into());
        let response = get(
            &format!("{BASE_URL}/artists/{artist_id}/tracks?page={page}&page-size={PAGE_SIZE}"),
            headers,
            http.user_agent.as_deref(),
        )?;
        if response.status_code() != 200 {
            return Err(anyhow!(
                "Yandex Music artist tracks returned HTTP {}",
                response.status_code()
            )
            .into());
        }
        let page_result: ArtistTracksResponse = response
            .json()
            .context("could not parse artist tracks JSON")?;
        let count = page_result.result.tracks.len();
        for track in page_result.result.tracks {
            if deferred_urls.len() == max_tracks {
                break;
            }
            let track_id = track
                .id
                .as_deref()
                .context("artist response contained a track without an ID")?;
            deferred_urls.push(format!("https://music.yandex.ru/track/{track_id}"));
        }
        if count < PAGE_SIZE {
            break;
        }
        page += 1;
    }
    if deferred_urls.is_empty() {
        return Err(anyhow!("artist response did not contain downloadable tracks").into());
    }
    Ok(Json(ResolveResponse {
        api_version: 1,
        candidates: Vec::new(),
        metadata: SourceMetadata::default(),
        items: Vec::new(),
        deferred_urls,
    }))
}

fn source_metadata(track: Track) -> SourceMetadata {
    let Track {
        title,
        version,
        artists,
        albums,
        cover_uri,
        genre,
        ..
    } = track;
    let album = albums.first();
    let title = match (title, version) {
        (Some(title), Some(version)) if !version.is_empty() => Some(format!("{title} ({version})")),
        (title, _) => title,
    };
    let artist = artist_names(&artists);
    let album_artist = album
        .and_then(|album| artist_names(&album.artists))
        .or_else(|| artist.clone());
    let artwork_uri = album
        .and_then(|album| album.cover_uri.as_deref())
        .or(cover_uri.as_deref());
    SourceMetadata {
        title,
        artist,
        album: album.and_then(|album| album.title.clone()),
        album_artist,
        track_number: album
            .and_then(|album| album.track_position.as_ref())
            .and_then(|position| position.index),
        disc_number: album
            .and_then(|album| album.track_position.as_ref())
            .and_then(|position| position.volume),
        date: album
            .and_then(|album| album.release_date.clone())
            .or_else(|| album.and_then(|album| album.year.map(|year| year.to_string()))),
        genre,
        artwork_url: artwork_uri.map(cover_url),
    }
}

fn artist_names(artists: &[Artist]) -> Option<String> {
    let names: Vec<&str> = artists
        .iter()
        .filter_map(|artist| artist.name.as_deref())
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

fn cover_url(uri: &str) -> String {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        uri.replace("%%", "1000x1000")
    } else {
        format!("https://{}", uri.replace("%%", "1000x1000"))
    }
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn get(
    url: &str,
    headers: BTreeMap<String, String>,
    user_agent: Option<&str>,
) -> FnResult<HttpResponse> {
    let mut request = HttpRequest::new(url).with_method("GET");
    for (name, value) in headers {
        request = request.with_header(name, value);
    }
    if let Some(user_agent) = user_agent {
        request = request.with_header("User-Agent", user_agent);
    }
    Ok(http::request::<()>(&request, None)?)
}

fn extract_track_id(value: &str) -> FnResult<&str> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(value);
    }
    for marker in ["/track/", "/tracks/"] {
        if let Some(after_marker) = value.split_once(marker).map(|(_, suffix)| suffix) {
            let track_id = after_marker
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default();
            if !track_id.is_empty() && track_id.bytes().all(|byte| byte.is_ascii_digit()) {
                return Ok(track_id);
            }
        }
    }
    Err(anyhow!("could not extract a track ID from the URL").into())
}

fn extract_album_id(value: &str) -> Option<&str> {
    if value.contains("/track/") || value.contains("/tracks/") {
        return None;
    }
    let (_, suffix) = value.split_once("/album/")?;
    let album_id = suffix.split(['/', '?', '#']).next()?;
    (!album_id.is_empty() && album_id.bytes().all(|byte| byte.is_ascii_digit())).then_some(album_id)
}

fn extract_artist_id(value: &str) -> Option<&str> {
    let (_, suffix) = value.split_once("/artist/")?;
    let artist_id = suffix.split(['/', '?', '#']).next()?;
    (!artist_id.is_empty() && artist_id.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some(artist_id)
}

fn xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let (_, value) = xml.split_once(&open)?;
    Some(value.split_once(&close)?.0.trim())
}
