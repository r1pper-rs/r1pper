# r1pper
> R1P EV3RYTH1NG!

`r1pper` is a modular audio downloader. Pronounces similar as reaper.

## Requirements

- FFmpeg available as `ffmpeg`, or supplied through `FfmpegRunner::new` when using the Rust API.
- Source plugins compiled as Extism-compatible WASM modules (not really required lol, only for additional sources).

## CLI

```sh
r1pper config # credential values are redacted
r1pper config --show-secrets
r1pper plugins list
r1pper plugins inspect example
r1pper search "artist track"
r1pper download https://example.test/track/42
```

Pass `--config path/to/r1pper.toml` to load settings. Without one, files are written under `downloads/` and plugins are discovered under `plugins/`.

## Configuration

```toml
output_directory = "downloads"
output_template = "{artist}/{album}/{track:02} - {title}.{ext}"
temporary_directory = ".tmp"
plugin_directories = ["plugins", "/opt/r1pper/plugins"]
concurrent_downloads = 2
overwrite = "rename" # overwrite, skip, or rename

[audio]
container = "opus"
codec = "opus"
bitrate_kbps = 160
sample_rate_hz = 48000
channels = 2
normalize_loudness = true
embed_artwork = true

[plugin_config.example]
preferred_quality = "high"

[http.example]
user_agent = "r1pper/0.1"
headers = { "X-Client" = "desktop" }

[auth.example]
scheme = "oauth"
access_token = "secret-token"
refresh_token = "refresh-token"
```

`bitrate_kbps` and `quality` are mutually exclusive. Valid container/codec combinations are `mp3/mp3`, `m4a/aac`, `opus/opus`, `flac/flac`, and `wav/pcm_s16le`.

`output_template` is relative to `output_directory` and creates its parent directories automatically. Available placeholders are `{artist}`, `{album_artist}`, `{album}`, `{title}`, `{track}`, `{track:02}`, `{disc}`, and `{ext}`. It must contain `{ext}`. For a flat layout, use `"{artist} - {title}.{ext}"`.

## Built-in plugins
### Spotify

Spotify is a built-in native plugin. It supports Spotify track, album, and playlist URLs and URIs:

```sh
r1pper auth login spotify
r1pper download https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC
r1pper download spotify:track:4uLU6hMCjMI75M1A2tKUQC
```


### Soulseek

Soulseek is a built-in native plugin. Configure a Soulseek account:

```toml
[auth.soulseek]
username = "your-username"
password = "your-password"
```

Search both built-ins by default, or select one or more with `--plugin`:

```sh
r1pper search "artist track"
r1pper search --plugin soulseek "artist track"
```

Each Soulseek result is printed as an exact `soulseek://` URL that can be downloaded directly.
