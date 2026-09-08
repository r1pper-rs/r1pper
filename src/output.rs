use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{AudioProfile, Error, OverwritePolicy, Result, plugin::SourceMetadata};

pub(crate) enum OutputTarget {
    Write(PathBuf),
    Skip(PathBuf),
}

pub(crate) fn target(
    directory: &Path,
    template: &str,
    metadata: &SourceMetadata,
    profile: &AudioProfile,
    policy: OverwritePolicy,
) -> Result<OutputTarget> {
    fs::create_dir_all(directory).map_err(|source| Error::CreateOutputDirectory {
        path: directory.to_path_buf(),
        source,
    })?;
    let extension = match profile.container {
        crate::config::AudioContainer::Mp3 => "mp3",
        crate::config::AudioContainer::M4a => "m4a",
        crate::config::AudioContainer::Opus => "opus",
        crate::config::AudioContainer::Flac => "flac",
        crate::config::AudioContainer::Wav => "wav",
    };
    let relative = render_template(template, metadata, extension);
    let candidate = directory.join(relative);
    let parent = candidate.parent().unwrap_or(directory);
    fs::create_dir_all(parent).map_err(|source| Error::CreateOutputDirectory {
        path: parent.to_path_buf(),
        source,
    })?;
    if !candidate.exists() {
        return Ok(OutputTarget::Write(candidate));
    }
    match policy {
        OverwritePolicy::Overwrite => Ok(OutputTarget::Write(candidate)),
        OverwritePolicy::Skip => Ok(OutputTarget::Skip(candidate)),
        OverwritePolicy::Rename => {
            let stem = candidate
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("audio");
            let extension = candidate
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or(extension);
            for index in 2.. {
                let renamed = parent.join(format!("{stem} ({index}).{extension}"));
                if !renamed.exists() {
                    return Ok(OutputTarget::Write(renamed));
                }
            }
            unreachable!("unbounded range always yields a filename")
        }
    }
}

fn render_template(template: &str, metadata: &SourceMetadata, extension: &str) -> PathBuf {
    let artist = metadata
        .artist
        .as_deref()
        .map(sanitize)
        .unwrap_or_else(|| "Unknown Artist".into());
    let album_artist = metadata
        .album_artist
        .as_deref()
        .map(sanitize)
        .unwrap_or_else(|| artist.clone());
    let album = metadata
        .album
        .as_deref()
        .map(sanitize)
        .unwrap_or_else(|| "Unknown Album".into());
    let title = metadata
        .title
        .as_deref()
        .map(sanitize)
        .unwrap_or_else(|| "audio".into());
    let track = metadata
        .track_number
        .map(|value| value.to_string())
        .unwrap_or_else(|| "00".into());
    let track_padded = metadata
        .track_number
        .map(|value| format!("{value:02}"))
        .unwrap_or_else(|| "00".into());
    let disc = metadata
        .disc_number
        .map(|value| value.to_string())
        .unwrap_or_else(|| "0".into());
    let mut rendered = template.replace("{track:02}", &track_padded);
    for (placeholder, value) in [
        ("{artist}", artist),
        ("{album_artist}", album_artist),
        ("{album}", album),
        ("{title}", title),
        ("{track}", track),
        ("{disc}", disc),
        ("{ext}", extension.into()),
    ] {
        rendered = rendered.replace(placeholder, &value);
    }
    PathBuf::from(rendered)
}

pub(crate) fn sanitize(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect();
    let sanitized = sanitized.trim_matches([' ', '.']);
    if sanitized.is_empty() {
        "audio".into()
    } else {
        sanitized.chars().take(180).collect()
    }
}
