# Yandex Music Source Plugin

## Build and Install

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/r1pper_yandex_music_plugin.wasm yandex_music.wasm
cp yandex_music.plugin.toml ../../plugins/
cp yandex_music.wasm ../../plugins/
```

Login after installation:

```sh
r1pper auth login yandex_music
```

Credentials saved to `r1pper.toml`:

```toml
[auth.yandex_music]
scheme = "oauth"
access_token = "your-yandex-oauth-token"

[http.yandex_music]
user_agent = "r1pper-yandex-music-plugin/0.1"
headers = { "X-Client" = "r1pper" }
```

## Search

```sh
r1pper search --plugin yandex_music "artist track"
```

## Albums and Artists

```toml
[plugin_config.yandex_music]
artist_max_tracks = 100 # Default: 10. Maximum: 1000.
```
