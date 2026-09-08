use std::{
    io::{Read, Write},
    path::Path,
    thread,
    time::Duration,
};

use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use tempfile::{Builder, NamedTempFile};

use crate::{Error, Result, config::HttpSettings, plugin::MediaCandidate};

pub(crate) fn fetch(
    candidate: &MediaCandidate,
    temp_dir: Option<&Path>,
    settings: &HttpSettings,
    mut progress: Option<&mut dyn FnMut(u64, Option<u64>)>,
) -> Result<NamedTempFile> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    let mut headers = HeaderMap::new();
    for (name, value) in &settings.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| Error::Message(format!("invalid configured header name: {error}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|error| Error::Message(format!("invalid configured header value: {error}")))?;
        headers.insert(name, value);
    }
    for (name, value) in &candidate.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| Error::Message(format!("invalid plugin header name: {error}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|error| Error::Message(format!("invalid plugin header value: {error}")))?;
        headers.insert(name, value);
    }
    let mut response = None;
    for attempt in 0..3 {
        let mut request = client.get(&candidate.url).headers(headers.clone());
        if let Some(user_agent) = &settings.user_agent {
            request = request.header(reqwest::header::USER_AGENT, user_agent);
        }
        match request
            .send()
            .and_then(|response| response.error_for_status())
        {
            Ok(result) => {
                response = Some(result);
                break;
            }
            Err(_) if attempt < 2 => thread::sleep(Duration::from_secs(1 << attempt)),
            Err(error) => return Err(Error::Network(error)),
        }
    }
    let mut response = response.expect("a successful request exits the retry loop");
    let total = response.content_length();
    let mut file = match temp_dir {
        Some(directory) => {
            std::fs::create_dir_all(directory).map_err(Error::TemporaryFile)?;
            Builder::new().prefix("r1pper-").tempfile_in(directory)
        }
        None => Builder::new().prefix("r1pper-").tempfile(),
    }
    .map_err(Error::TemporaryFile)?;
    let mut downloaded = 0;
    if let Some(callback) = progress.as_deref_mut() {
        callback(downloaded, total);
    }
    let mut buffer = [0; 64 * 1024];
    loop {
        let bytes = response.read(&mut buffer).map_err(Error::TemporaryFile)?;
        if bytes == 0 {
            break;
        }
        file.write_all(&buffer[..bytes])
            .map_err(Error::TemporaryFile)?;
        downloaded += bytes as u64;
        if let Some(callback) = progress.as_deref_mut() {
            callback(downloaded, total);
        }
    }
    Ok(file)
}
