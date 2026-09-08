use std::{
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::Builder;

use crate::{AudioProfile, Error, Result, plugin::SourceMetadata};

#[derive(Clone, Debug)]
pub struct FfmpegRunner {
    executable: PathBuf,
}

impl Default for FfmpegRunner {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("ffmpeg"),
        }
    }
}

impl FfmpegRunner {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    pub fn verify(&self) -> Result<()> {
        let output = Command::new(&self.executable)
            .arg("-version")
            .output()
            .map_err(|source| Error::StartFfmpeg {
                path: self.executable.clone(),
                source,
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Error::FfmpegFailed(
                String::from_utf8_lossy(&output.stderr).trim().into(),
            ))
        }
    }

    pub fn transcode(
        &self,
        input: &Path,
        destination: &Path,
        profile: &AudioProfile,
        metadata: &SourceMetadata,
        artwork: Option<&Path>,
    ) -> Result<()> {
        let extension = destination
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("audio");
        let temporary = Builder::new()
            .prefix(".r1pper-")
            .suffix(&format!(".{extension}"))
            .tempfile_in(destination.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(Error::TemporaryFile)?;
        let temp_path = temporary.path().to_path_buf();
        drop(temporary);
        let mut command = Command::new(&self.executable);
        command.arg("-nostdin").arg("-y").arg("-i").arg(input);
        if let Some(artwork) = artwork {
            command
                .arg("-i")
                .arg(artwork)
                .arg("-map")
                .arg("0:a:0")
                .arg("-map")
                .arg("1:v:0")
                .arg("-c:v")
                .arg("mjpeg")
                .arg("-disposition:v")
                .arg("attached_pic");
        } else {
            command.arg("-vn");
        }
        command.arg("-c:a").arg(match profile.codec {
            crate::config::AudioCodec::Mp3 => "libmp3lame",
            crate::config::AudioCodec::Aac => "aac",
            crate::config::AudioCodec::Opus => "libopus",
            crate::config::AudioCodec::Flac => "flac",
            crate::config::AudioCodec::PcmS16le => "pcm_s16le",
        });
        if let Some(bitrate) = profile.bitrate_kbps {
            command.arg("-b:a").arg(format!("{bitrate}k"));
        }
        if let Some(quality) = profile.quality {
            command.arg("-q:a").arg(quality.to_string());
        }
        if let Some(rate) = profile.sample_rate_hz {
            command.arg("-ar").arg(rate.to_string());
        }
        if let Some(channels) = profile.channels {
            command.arg("-ac").arg(channels.to_string());
        }
        if profile.normalize_loudness {
            command.arg("-af").arg("loudnorm=I=-16:TP=-1.5:LRA=11");
        }
        let mut tags = vec![
            ("title", metadata.title.clone()),
            ("artist", metadata.artist.clone()),
            ("album", metadata.album.clone()),
            ("album_artist", metadata.album_artist.clone()),
            ("date", metadata.date.clone()),
            ("genre", metadata.genre.clone()),
        ];
        if let Some(track_number) = metadata.track_number {
            tags.push(("track", Some(track_number.to_string())));
        }
        if let Some(disc_number) = metadata.disc_number {
            tags.push(("disc", Some(disc_number.to_string())));
        }
        for (key, value) in tags {
            if let Some(value) = value {
                command.arg("-metadata").arg(format!("{key}={value}"));
            }
        }
        let output = command
            .arg(&temp_path)
            .output()
            .map_err(|source| Error::StartFfmpeg {
                path: self.executable.clone(),
                source,
            })?;
        if !output.status.success() {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Error::FfmpegFailed(
                String::from_utf8_lossy(&output.stderr).trim().into(),
            ));
        }
        std::fs::rename(&temp_path, destination).map_err(|source| Error::FinalizeOutput {
            path: destination.to_path_buf(),
            source,
        })
    }
}
