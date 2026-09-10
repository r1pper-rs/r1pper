use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use iced::{
    Alignment, Background, Border, Color, Element, Fill, Font, Task, Theme,
    widget::{button, column, container, image, pick_list, row, scrollable, text, text_input},
};
use r1pper::{
    Config, DownloadEvent, DownloadRequest, Downloader, OverwritePolicy,
    auth::AuthCredentials,
    config::{AudioCodec, AudioContainer},
    plugin::PluginRegistry,
};

const DEFAULT_CONFIG_PATH: &str = "r1pper.toml";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const CORNER_RADIUS: f32 = 4.0;
const BACKGROUND: Color = Color::from_rgb(0.055, 0.086, 0.071);
const PANEL: Color = Color::from_rgb(0.082, 0.125, 0.102);
const PANEL_HOVER: Color = Color::from_rgb(0.123, 0.180, 0.145);
const GREEN: Color = Color::from_rgb(0.486, 0.765, 0.588);
const TEXT: Color = Color::from_rgb(0.858, 0.929, 0.870);
const MUTED: Color = Color::from_rgb(0.637, 0.749, 0.659);

fn main() -> iced::Result {
    iced::application("r1pper", R1pper::update, R1pper::view)
        .theme(|_| {
            Theme::custom(
                "R1pper Fern".into(),
                iced::theme::Palette {
                    background: BACKGROUND,
                    text: TEXT,
                    primary: GREEN,
                    success: Color::from_rgb(0.607, 0.831, 0.647),
                    danger: Color::from_rgb(0.878, 0.494, 0.482),
                },
            )
        })
        .window(iced::window::Settings {
            size: iced::Size::new(500.0, 550.0),
            min_size: Some(iced::Size::new(420.0, 460.0)),
            platform_specific: {
              #[cfg(target_os = "macos")]
              {
                iced::window::settings::PlatformSpecific {
                 title_hidden: true,
                 titlebar_transparent: true,
                 fullsize_content_view: true,
                }
              }
              #[cfg(not(target_os = "macos"))]
              {
                  iced::window::settings::PlatformSpecific::default()
              }
            },
            ..Default::default()
        })
        .default_font(Font::MONOSPACE)
        .subscription(|_| iced::time::every(Duration::from_millis(150)).map(|_| Message::Tick))
        .run_with(R1pper::new)
}

#[derive(Default)]
struct R1pper {
    url: String,
    config_path: String,
    overwrite: bool,
    status: String,
    results: Vec<String>,
    downloading: bool,
    screen: Screen,
    plugins: Vec<String>,
    search_sources: Vec<String>,
    search_source: String,
    search_results: Vec<ResultItem>,
    selected_results: BTreeSet<String>,
    artworks: BTreeMap<String, image::Handle>,
    progress: Arc<Mutex<String>>,
    mode: QueryMode,
    output_directory: String,
    concurrent_downloads: String,
    audio_format: AudioFormat,
    embed_artwork: bool,
    normalize_loudness: bool,
    auth_plugin: String,
    auth_token: String,
    auth_message: String,
    authenticating: bool,
    spotify_login: Option<Arc<Mutex<Option<r1pper::spotify::DeviceLogin>>>>,
    plugin_login: Option<Arc<Mutex<Option<r1pper::auth::DeviceLogin>>>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum AudioFormat {
    Mp3,
    M4a,
    #[default]
    Opus,
    Flac,
    Wav,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum QueryMode {
    Search,
    #[default]
    Download,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Screen {
    #[default]
    Main,
    Config,
    Plugins,
    Auth,
}

const ALL_SOURCES: &str = "All sources";

#[derive(Debug, Clone)]
struct ResultItem {
    source: String,
    title: String,
    artist: String,
    url: String,
    artwork_url: Option<String>,
}

#[derive(Debug, Clone)]
struct SearchOutput {
    results: Vec<ResultItem>,
    logs: Vec<String>,
}

impl QueryMode {
    fn label(self) -> &'static str {
        match self {
            Self::Search => "SEARCH",
            Self::Download => "DOWNLOAD",
        }
    }
}

impl AudioFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::M4a => "M4A",
            Self::Opus => "OPUS",
            Self::Flac => "FLAC",
            Self::Wav => "WAV",
        }
    }

    fn apply(self, config: &mut Config) {
        (config.audio.container, config.audio.codec) = match self {
            Self::Mp3 => (AudioContainer::Mp3, AudioCodec::Mp3),
            Self::M4a => (AudioContainer::M4a, AudioCodec::Aac),
            Self::Opus => (AudioContainer::Opus, AudioCodec::Opus),
            Self::Flac => (AudioContainer::Flac, AudioCodec::Flac),
            Self::Wav => (AudioContainer::Wav, AudioCodec::PcmS16le),
        };
    }

    fn from_config(config: &Config) -> Self {
        match config.audio.container {
            AudioContainer::Mp3 => Self::Mp3,
            AudioContainer::M4a => Self::M4a,
            AudioContainer::Opus => Self::Opus,
            AudioContainer::Flac => Self::Flac,
            AudioContainer::Wav => Self::Wav,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    UrlChanged(String),
    PasteQuery,
    ClipboardRead(Option<String>),
    ModeSelected(QueryMode),
    SearchSourceSelected(String),
    ToggleResult(String),
    DownloadSelected,
    SelectedDownloadsFinished(Result<Vec<String>, String>),
    ArtworkLoaded(String, Result<Vec<u8>, String>),
    Tick,
    ConfigPathChanged(String),
    OverwriteChanged(bool),
    ToggleConfig,
    TogglePlugins,
    ToggleAuth,
    AuthPluginChanged(String),
    AuthTokenChanged(String),
    StartSpotifyLogin,
    SpotifyLoginStarted(Result<Arc<Mutex<Option<r1pper::spotify::DeviceLogin>>>, String>),
    FinishSpotifyLogin,
    SpotifyLoginFinished(Result<(), String>),
    StartPluginLogin,
    PluginLoginStarted(Result<Arc<Mutex<Option<r1pper::auth::DeviceLogin>>>, String>),
    FinishPluginLogin,
    PluginLoginFinished(Result<(), String>),
    SaveToken,
    TokenSaved(Result<(), String>),
    OutputDirectoryChanged(String),
    ConcurrentDownloadsChanged(String),
    AudioFormatSelected(AudioFormat),
    EmbedArtworkChanged(bool),
    NormalizeLoudnessChanged(bool),
    SaveConfig,
    ConfigSaved(Result<(), String>),
    ReloadConfig,
    Run,
    DownloadFinished(Result<Vec<String>, String>),
    SearchFinished(Result<SearchOutput, String>),
}

impl R1pper {
    fn new() -> (Self, Task<Message>) {
        let config_path = default_config_path();
        let config = load_config(&config_path.display().to_string()).unwrap_or_default();
        (
            Self {
                config_path: config_path.display().to_string(),
                status: "Enter a source URL to begin.".into(),
                overwrite: matches!(config.overwrite, OverwritePolicy::Overwrite),
                output_directory: config.output_directory.display().to_string(),
                concurrent_downloads: config.concurrent_downloads.to_string(),
                audio_format: AudioFormat::from_config(&config),
                embed_artwork: config.audio.embed_artwork,
                normalize_loudness: config.audio.normalize_loudness,
                plugins: plugin_list(&config),
                search_sources: search_sources(&config),
                search_source: ALL_SOURCES.into(),
                auth_plugin: "yandex_music".into(),
                auth_message: "Connect Spotify or save an access token.".into(),
                ..Self::default()
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::UrlChanged(url) => self.url = url,
            Message::Tick => {}
            Message::PasteQuery => {
                return iced::clipboard::read().map(Message::ClipboardRead);
            }
            Message::ClipboardRead(Some(contents)) => self.url = contents.trim().into(),
            Message::ClipboardRead(None) => self.status = "Clipboard is empty.".into(),
            Message::ModeSelected(mode) => self.mode = mode,
            Message::SearchSourceSelected(source) => self.search_source = source,
            Message::ToggleResult(url) => {
                if !self.selected_results.insert(url.clone()) {
                    self.selected_results.remove(&url);
                }
            }
            Message::DownloadSelected => {
                let urls = self.selected_results.iter().cloned().collect::<Vec<_>>();
                if urls.is_empty() {
                    self.results = vec!["Select one or more results first.".into()];
                    return Task::none();
                }
                self.downloading = true;
                let config_path = self.config_path.clone();
                let overwrite = self.overwrite;
                let progress = self.progress.clone();
                set_progress(&progress, "Preparing selected downloads...".into());
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            download_selected(urls, config_path, overwrite, &progress)
                        })
                        .await
                        .map_err(|error| format!("Download task failed: {error}"))?
                    },
                    Message::SelectedDownloadsFinished,
                );
            }
            Message::SelectedDownloadsFinished(result) => {
                self.downloading = false;
                set_progress(&self.progress, String::new());
                self.results = result.unwrap_or_else(|error| vec![format!("Error: {error}")]);
            }
            Message::ArtworkLoaded(url, Ok(bytes)) => {
                self.artworks.insert(url, image::Handle::from_bytes(bytes));
            }
            Message::ArtworkLoaded(_, Err(_)) => {}
            Message::ConfigPathChanged(path) => self.config_path = path,
            Message::OverwriteChanged(overwrite) => self.overwrite = overwrite,
            Message::ToggleConfig => {
                self.screen = if self.screen == Screen::Config {
                    Screen::Main
                } else {
                    Screen::Config
                }
            }
            Message::TogglePlugins => {
                self.screen = if self.screen == Screen::Plugins {
                    Screen::Main
                } else {
                    Screen::Plugins
                }
            }
            Message::ToggleAuth => {
                self.screen = if self.screen == Screen::Auth {
                    Screen::Main
                } else {
                    Screen::Auth
                }
            }
            Message::AuthPluginChanged(plugin) => self.auth_plugin = plugin,
            Message::AuthTokenChanged(token) => self.auth_token = token,
            Message::StartSpotifyLogin => {
                self.authenticating = true;
                self.auth_message = "Requesting Spotify device code...".into();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(|| {
                            r1pper::spotify::begin_login()
                                .map(|login| Arc::new(Mutex::new(Some(login))))
                        })
                        .await
                        .map_err(|error| format!("Authorization task failed: {error}"))?
                        .map_err(|error| error.to_string())
                    },
                    Message::SpotifyLoginStarted,
                );
            }
            Message::SpotifyLoginStarted(result) => {
                self.authenticating = false;
                match result {
                    Ok(login) => {
                        self.auth_message = "Open the URL, enter the code, then finish.".into();
                        self.spotify_login = Some(login);
                    }
                    Err(error) => {
                        self.auth_message = format!("Spotify authorization failed: {error}")
                    }
                }
            }
            Message::FinishSpotifyLogin => {
                let Some(login) = self
                    .spotify_login
                    .take()
                    .and_then(|login| login.lock().ok().and_then(|mut login| login.take()))
                else {
                    self.auth_message = "Request a Spotify code first.".into();
                    return Task::none();
                };
                self.authenticating = true;
                self.auth_message = "Waiting for Spotify approval...".into();
                let config_path = PathBuf::from(self.config_path.trim());
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            r1pper::spotify::finish_login(login, &config_path)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| format!("Authorization task failed: {error}"))?
                    },
                    Message::SpotifyLoginFinished,
                );
            }
            Message::SpotifyLoginFinished(result) => {
                self.authenticating = false;
                self.auth_message = match result {
                    Ok(()) => "Spotify connected. Credentials saved.".into(),
                    Err(error) => format!("Spotify authorization failed: {error}"),
                };
            }
            Message::StartPluginLogin => {
                self.authenticating = true;
                self.auth_message = "Requesting device code...".into();
                let config_path = self.config_path.clone();
                let plugin = self.auth_plugin.trim().to_owned();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            begin_plugin_login(&config_path, &plugin)
                                .map(|login| Arc::new(Mutex::new(Some(login))))
                        })
                        .await
                        .map_err(|error| format!("Authorization task failed: {error}"))?
                    },
                    Message::PluginLoginStarted,
                );
            }
            Message::PluginLoginStarted(result) => {
                self.authenticating = false;
                match result {
                    Ok(login) => {
                        self.auth_message = "Open the URL, enter the code, then finish.".into();
                        self.plugin_login = Some(login);
                    }
                    Err(error) => self.auth_message = format!("Authorization failed: {error}"),
                }
            }
            Message::FinishPluginLogin => {
                let Some(login) = self
                    .plugin_login
                    .take()
                    .and_then(|login| login.lock().ok().and_then(|mut login| login.take()))
                else {
                    self.auth_message = "Request a device code first.".into();
                    return Task::none();
                };
                self.authenticating = true;
                self.auth_message = "Waiting for approval...".into();
                let config_path = PathBuf::from(self.config_path.trim());
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            r1pper::auth::finish_device_login(login, &config_path)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| format!("Authorization task failed: {error}"))?
                    },
                    Message::PluginLoginFinished,
                );
            }
            Message::PluginLoginFinished(result) => {
                self.authenticating = false;
                self.auth_message = match result {
                    Ok(()) => "Plugin connected. Credentials saved.".into(),
                    Err(error) => format!("Authorization failed: {error}"),
                };
            }
            Message::SaveToken => {
                let config_path = PathBuf::from(self.config_path.trim());
                let plugin = self.auth_plugin.trim().to_owned();
                let token = self.auth_token.trim().to_owned();
                if plugin.is_empty() || token.is_empty() {
                    self.auth_message = "Plugin ID and token are required.".into();
                    return Task::none();
                }
                self.auth_message = "Saving token...".into();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            save_access_token(config_path, plugin, token)
                        })
                        .await
                        .map_err(|error| format!("Authorization task failed: {error}"))?
                    },
                    Message::TokenSaved,
                );
            }
            Message::TokenSaved(result) => {
                self.auth_message = match result {
                    Ok(()) => "Token saved.".into(),
                    Err(error) => format!("Token save failed: {error}"),
                };
            }
            Message::OutputDirectoryChanged(path) => self.output_directory = path,
            Message::ConcurrentDownloadsChanged(count) => self.concurrent_downloads = count,
            Message::AudioFormatSelected(format) => self.audio_format = format,
            Message::EmbedArtworkChanged(embed) => self.embed_artwork = embed,
            Message::NormalizeLoudnessChanged(normalize) => self.normalize_loudness = normalize,
            Message::ReloadConfig => match load_config(self.config_path.trim()) {
                Ok(config) => self.apply_config(config),
                Err(error) => self.status = format!("Error: {error}"),
            },
            Message::SaveConfig => {
                let path = self.config_path.trim().to_owned();
                let output_directory = self.output_directory.trim().to_owned();
                let concurrent_downloads = self.concurrent_downloads.trim().to_owned();
                let audio_format = self.audio_format;
                let embed_artwork = self.embed_artwork;
                let normalize_loudness = self.normalize_loudness;
                self.status = "Saving configuration...".into();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            save_config(
                                path,
                                output_directory,
                                concurrent_downloads,
                                audio_format,
                                embed_artwork,
                                normalize_loudness,
                            )
                        })
                        .await
                        .map_err(|error| format!("Configuration task failed: {error}"))?
                    },
                    Message::ConfigSaved,
                );
            }
            Message::ConfigSaved(result) => match result {
                Ok(()) => self.status = "Configuration saved.".into(),
                Err(error) => self.status = format!("Error: {error}"),
            },
            Message::Run => {
                let query = self.url.trim().to_owned();
                if query.is_empty() {
                    self.status = "Enter a search or URL first.".into();
                    self.results = vec![self.status.clone()];
                    return Task::none();
                }

                self.downloading = true;
                self.status = format!("{}...", self.mode.label().to_lowercase());
                self.results = vec![self.status.clone()];
                set_progress(&self.progress, self.status.clone());
                let config_path = self.config_path.trim().to_owned();
                let search_source = self.search_source.clone();
                let overwrite = self.overwrite;
                let progress = self.progress.clone();
                return match self.mode {
                    QueryMode::Download => Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                download(query, config_path, overwrite, &progress)
                            })
                            .await
                            .map_err(|error| format!("Download task failed: {error}"))?
                        },
                        Message::DownloadFinished,
                    ),
                    QueryMode::Search => Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                search(query, config_path, search_source)
                            })
                            .await
                            .map_err(|error| format!("Search task failed: {error}"))?
                        },
                        Message::SearchFinished,
                    ),
                };
            }
            Message::DownloadFinished(result) => {
                self.downloading = false;
                set_progress(&self.progress, String::new());
                match result {
                    Ok(results) => {
                        self.status = format!("Completed {} download(s).", results.len());
                        self.results = results;
                    }
                    Err(error) => {
                        self.status = format!("Error: {error}");
                        self.results = vec![self.status.clone()];
                    }
                }
            }
            Message::SearchFinished(result) => {
                self.downloading = false;
                set_progress(&self.progress, String::new());
                match result {
                    Ok(results) => {
                        self.status = format!("Found {} result(s).", results.results.len());
                        self.results = results.logs;
                        self.search_results = results.results;
                        self.selected_results.clear();
                        self.artworks.clear();
                        return Task::batch(self.search_results.iter().filter_map(|result| {
                            result.artwork_url.clone().map(|url| {
                                Task::perform(fetch_artwork(url.clone()), move |result| {
                                    Message::ArtworkLoaded(url.clone(), result)
                                })
                            })
                        }));
                    }
                    Err(error) => {
                        self.status = format!("Error: {error}");
                        self.results = vec![self.status.clone()];
                    }
                }
            }
        }
        Task::none()
    }

    fn apply_config(&mut self, config: Config) {
        self.overwrite = matches!(config.overwrite, OverwritePolicy::Overwrite);
        self.output_directory = config.output_directory.display().to_string();
        self.concurrent_downloads = config.concurrent_downloads.to_string();
        self.audio_format = AudioFormat::from_config(&config);
        self.embed_artwork = config.audio.embed_artwork;
        self.normalize_loudness = config.audio.normalize_loudness;
        self.plugins = plugin_list(&config);
        self.status = "Configuration loaded.".into();
    }

    fn view(&self) -> Element<'_, Message> {
        let source_input = text_input("Search music or paste a URL", &self.url)
            .on_input(Message::UrlChanged)
            .on_submit(Message::Run)
            .padding([14, 16])
            .size(14)
            .width(Fill);

        let config_input = text_input(DEFAULT_CONFIG_PATH, &self.config_path)
            .on_input(Message::ConfigPathChanged)
            .padding([9, 10])
            .size(12)
            .width(Fill);

        let run_button = button(
            text(if self.downloading {
                "WORKING..."
            } else {
                self.mode.label()
            })
            .size(12),
        )
        .on_press_maybe((!self.downloading).then_some(Message::Run))
        .padding([11, 22])
        .style(primary_button);

        let source_picker: Element<'_, Message> = if self.mode == QueryMode::Search {
            pick_list(
                self.search_sources.clone(),
                Some(self.search_source.clone()),
                Message::SearchSourceSelected,
            )
            .placeholder(ALL_SOURCES)
            .width(Fill)
            .into()
        } else {
            container(text("")).into()
        };

        let progress_indicator: Element<'_, Message> = self
            .progress
            .lock()
            .ok()
            .filter(|progress| !progress.is_empty())
            .map(|progress| text(progress.clone()).size(10).color(GREEN).into())
            .unwrap_or_else(|| container(text("")).into());

        let form = column![
            text("r1pper").size(30).color(GREEN),
            row![
                source_input,
                button(text("PASTE").size(10))
                    .on_press(Message::PasteQuery)
                    .padding([10, 11])
                    .style(secondary_button),
            ]
            .spacing(6),
            row![
                mode_button(QueryMode::Search, self.mode),
                mode_button(QueryMode::Download, self.mode),
            ]
            .spacing(6),
            source_picker,
            run_button,
            progress_indicator,
        ]
        .align_x(Alignment::Center)
        .spacing(10)
        .max_width(620);

        let configuration = if self.screen == Screen::Config {
            column![
                row![
                    text("CONFIGURATION").size(11).color(GREEN).width(Fill),
                    button(text("HIDE").size(10))
                        .on_press(Message::ToggleConfig)
                        .padding([6, 9])
                        .style(secondary_button),
                ]
                .align_y(Alignment::Center),
                text("Configuration file").size(11).color(MUTED),
                row![
                    config_input,
                    button(text("LOAD").size(10))
                        .on_press(Message::ReloadConfig)
                        .padding([9, 10])
                        .style(secondary_button),
                ]
                .spacing(8),
                text("Output directory").size(11).color(MUTED),
                text_input("downloads", &self.output_directory)
                    .on_input(Message::OutputDirectoryChanged)
                    .padding([9, 10])
                    .size(12),
                row![
                    column![
                        text("PARALLEL DOWNLOADS").size(10).color(MUTED),
                        text_input("2", &self.concurrent_downloads)
                            .on_input(Message::ConcurrentDownloadsChanged)
                            .padding([8, 9])
                            .size(12)
                    ]
                    .spacing(5)
                    .width(Fill),
                    column![
                        text("FILE COLLISION").size(10).color(MUTED),
                        iced::widget::checkbox("Overwrite", self.overwrite)
                            .on_toggle(Message::OverwriteChanged)
                    ]
                    .spacing(7)
                    .width(Fill),
                ]
                .spacing(14),
                text("AUDIO FORMAT").size(10).color(MUTED),
                row![
                    format_button(AudioFormat::Mp3, self.audio_format),
                    format_button(AudioFormat::M4a, self.audio_format),
                    format_button(AudioFormat::Opus, self.audio_format),
                    format_button(AudioFormat::Flac, self.audio_format),
                    format_button(AudioFormat::Wav, self.audio_format),
                ]
                .spacing(5),
                row![
                    iced::widget::checkbox("Embed artwork", self.embed_artwork)
                        .on_toggle(Message::EmbedArtworkChanged)
                        .width(Fill),
                    iced::widget::checkbox("Normalize loudness", self.normalize_loudness)
                        .on_toggle(Message::NormalizeLoudnessChanged)
                        .width(Fill),
                ],
                button(text("SAVE CONFIGURATION").size(11))
                    .on_press(Message::SaveConfig)
                    .padding([10, 14])
                    .width(Fill)
                    .style(primary_button),
            ]
            .spacing(9)
        } else {
            column![
                row![
                    text(&self.config_path).size(11).color(MUTED).width(Fill),
                    button(text("OPEN CONFIG").size(10))
                        .on_press(Message::ToggleConfig)
                        .padding([7, 10])
                        .style(secondary_button),
                ]
                .align_y(Alignment::Center),
            ]
        };

        let plugins = if self.screen == Screen::Plugins {
            let list = self.plugins.iter().fold(
                column![
                    row![
                        text("PLUGINS").size(11).color(GREEN).width(Fill),
                        button(text("HIDE").size(10))
                            .on_press(Message::TogglePlugins)
                            .padding([6, 9])
                            .style(secondary_button),
                    ]
                    .align_y(Alignment::Center)
                ]
                .spacing(7),
                |list, plugin| {
                    list.push(
                        container(text(plugin).size(11))
                            .padding([7, 9])
                            .style(panel),
                    )
                },
            );
            column![list]
        } else {
            column![
                row![
                    text("Plugin sources").size(11).color(MUTED).width(Fill),
                    button(text("PLUGINS").size(10))
                        .on_press(Message::TogglePlugins)
                        .padding([7, 10])
                        .style(secondary_button),
                ]
                .align_y(Alignment::Center),
            ]
        };

        let output = self.results.iter().fold(
            column![text("LOG").size(10).color(GREEN)].spacing(7),
            |list, path| list.push(text(path).size(12)),
        );

        let result_cards = self.search_results.iter().fold(
            column![
                row![
                    text(format!("{} selected", self.selected_results.len()))
                        .size(10)
                        .color(MUTED)
                        .width(Fill),
                    button(text("DOWNLOAD SELECTED").size(10))
                        .on_press_maybe((!self.downloading).then_some(Message::DownloadSelected))
                        .padding([8, 10])
                        .style(primary_button),
                ]
                .align_y(Alignment::Center)
            ]
            .spacing(7),
            |cards, result| {
                let artwork: Element<'_, Message> = result
                    .artwork_url
                    .as_ref()
                    .and_then(|url| self.artworks.get(url))
                    .map(|handle| image(handle.clone()).width(42).height(42).into())
                    .unwrap_or_else(|| {
                        container(text(&result.source).size(9).color(GREEN))
                            .width(42)
                            .height(42)
                            .center(Fill)
                            .style(panel)
                            .into()
                    });
                cards.push(
                    button(
                        row![
                            artwork,
                            column![
                                text(&result.title).size(12),
                                text(&result.artist).size(10).color(MUTED),
                                text(&result.source).size(9).color(GREEN),
                            ]
                            .spacing(2)
                            .width(Fill),
                        ]
                        .spacing(9)
                        .align_y(Alignment::Center),
                    )
                    .on_press(Message::ToggleResult(result.url.clone()))
                    .padding(8)
                    .width(Fill)
                    .height(64)
                    .style(if self.selected_results.contains(&result.url) {
                        primary_button
                    } else {
                        secondary_button
                    }),
                )
            },
        );

        let has_results = !self.search_results.is_empty();
        let main_area: Element<'_, Message> = if has_results {
            container(form).center_x(Fill).into()
        } else {
            container(form).center(Fill).height(Fill).into()
        };
        let results_panel: Element<'_, Message> = if has_results {
            scrollable(result_cards).height(300).into()
        } else {
            container(text("")).into()
        };

        let header = container(
            row![
                column![
                    row![
                        text(format!("v{APP_VERSION}"))
                            .size(14)
                            .color(Color { a: 0.62, ..MUTED }),
                    ]
                    .align_y(Alignment::Center),
                ]
                .spacing(2)
                .width(Fill),
                button(text("CONFIG").size(10))
                    .on_press(Message::ToggleConfig)
                    .padding([7, 10])
                    .style(secondary_button),
                button(text("PLUGINS").size(10))
                    .on_press(Message::TogglePlugins)
                    .padding([7, 10])
                    .style(secondary_button),
                button(text("AUTH").size(10))
                    .on_press(Message::ToggleAuth)
                    .padding([7, 10])
                    .style(secondary_button),
            ]
            .align_y(Alignment::Center)
            .spacing(7),
        )
        .padding([8, 0])
        .style(app_header);

        let spotify_code = self.spotify_login.as_ref().and_then(|login| {
            login.lock().ok().and_then(|login| {
                login
                    .as_ref()
                    .map(|login| (login.user_code.clone(), login.verification_url.clone()))
            })
        });
        let spotify_device: Element<'_, Message> =
            if let Some((user_code, verification_url)) = spotify_code {
                container(
                    column![
                        text("SPOTIFY CODE").size(10).color(GREEN),
                        text(user_code).size(28).color(TEXT),
                        text(verification_url).size(11).color(MUTED),
                        button(text("I ENTERED THE CODE").size(10))
                            .on_press_maybe(
                                (!self.authenticating).then_some(Message::FinishSpotifyLogin)
                            )
                            .padding([9, 12])
                            .width(Fill)
                            .style(primary_button),
                    ]
                    .spacing(7),
                )
                .padding(12)
                .style(panel)
                .into()
            } else {
                button(
                    text(if self.authenticating {
                        "REQUESTING SPOTIFY CODE..."
                    } else {
                        "CONNECT SPOTIFY"
                    })
                    .size(11),
                )
                .on_press_maybe((!self.authenticating).then_some(Message::StartSpotifyLogin))
                .padding([10, 14])
                .width(Fill)
                .style(primary_button)
                .into()
            };

        let plugin_code = self.plugin_login.as_ref().and_then(|login| {
            login.lock().ok().and_then(|login| {
                login
                    .as_ref()
                    .map(|login| (login.user_code.clone(), login.verification_url.clone()))
            })
        });
        let plugin_device: Element<'_, Message> =
            if let Some((user_code, verification_url)) = plugin_code {
                container(
                    column![
                        text("DEVICE CODE").size(10).color(GREEN),
                        text(user_code).size(28).color(TEXT),
                        text(verification_url).size(11).color(MUTED),
                        button(text("I ENTERED THE CODE").size(10))
                            .on_press_maybe(
                                (!self.authenticating).then_some(Message::FinishPluginLogin)
                            )
                            .padding([9, 12])
                            .width(Fill)
                            .style(primary_button),
                    ]
                    .spacing(7),
                )
                .padding(12)
                .style(panel)
                .into()
            } else {
                button(text("GET DEVICE CODE").size(10))
                    .on_press_maybe((!self.authenticating).then_some(Message::StartPluginLogin))
                    .padding([9, 12])
                    .width(Fill)
                    .style(secondary_button)
                    .into()
            };

        let auth = column![
            text("AUTHENTICATION").size(11).color(GREEN),
            text("Spotify device authorization").size(11).color(MUTED),
            spotify_device,
            text("Plugin authentication").size(11).color(MUTED),
            text_input("Plugin ID", &self.auth_plugin)
                .on_input(Message::AuthPluginChanged)
                .padding([9, 10])
                .size(12),
            text_input("Access token", &self.auth_token)
                .on_input(Message::AuthTokenChanged)
                .padding([9, 10])
                .size(12)
                .secure(true),
            plugin_device,
            button(text("SAVE TOKEN").size(11))
                .on_press(Message::SaveToken)
                .padding([10, 14])
                .width(Fill)
                .style(secondary_button),
            text(&self.auth_message).size(11).color(MUTED),
        ]
        .spacing(10);

        let content: Element<'_, Message> = match self.screen {
            Screen::Main => column![header, main_area, results_panel, container(output)]
                .spacing(16)
                .max_width(720)
                .into(),
            Screen::Config => column![header, container(configuration).padding(14).style(panel)]
                .spacing(16)
                .max_width(720)
                .into(),
            Screen::Plugins => column![header, container(plugins).padding(14).style(panel)]
                .spacing(16)
                .max_width(720)
                .into(),
            Screen::Auth => column![header, container(auth).padding(14).style(panel)]
                .spacing(16)
                .max_width(720)
                .into(),
        };

        container(content)
            .width(Fill)
            .height(Fill)
            .center_x(Fill)
            .padding(18)
            .into()
    }
}

fn panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: Border {
            color: Color::from_rgba(0.67, 0.90, 0.72, 0.10),
            width: 1.0,
            radius: CORNER_RADIUS.into(),
        },
        ..Default::default()
    }
}

fn app_header(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BACKGROUND)),
        ..Default::default()
    }
}

fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Active => GREEN,
        button::Status::Hovered => Color::from_rgb(0.62, 0.86, 0.67),
        button::Status::Pressed => Color::from_rgb(0.34, 0.60, 0.43),
        button::Status::Disabled => PANEL_HOVER,
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: BACKGROUND,
        border: Border {
            radius: CORNER_RADIUS.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn secondary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => PANEL_HOVER,
        _ => PANEL,
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: TEXT,
        border: Border {
            color: Color::from_rgba(0.67, 0.90, 0.72, 0.13),
            width: 1.0,
            radius: CORNER_RADIUS.into(),
        },
        ..Default::default()
    }
}

fn format_button(
    format: AudioFormat,
    selected: AudioFormat,
) -> iced::widget::Button<'static, Message> {
    button(text(format.label()).size(10))
        .on_press(Message::AudioFormatSelected(format))
        .padding([7, 8])
        .width(Fill)
        .style(if format == selected {
            primary_button
        } else {
            secondary_button
        })
}

fn mode_button(mode: QueryMode, selected: QueryMode) -> iced::widget::Button<'static, Message> {
    button(text(mode.label()).size(10))
        .on_press(Message::ModeSelected(mode))
        .padding([8, 14])
        .width(Fill)
        .style(if mode == selected {
            primary_button
        } else {
            secondary_button
        })
}

fn load_config(path: &str) -> Result<Config, String> {
    let path = PathBuf::from(path);
    if path.exists() {
        let mut config = Config::from_file(&path).map_err(|error| error.to_string())?;
        if let Some(parent) = path.parent() {
            for directory in &mut config.plugin_directories {
                if directory.is_relative() {
                    *directory = parent.join(&directory);
                }
            }
        }
        Ok(config)
    } else {
        Ok(Config::default())
    }
}

fn default_config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(DEFAULT_CONFIG_PATH)
}

fn plugin_list(config: &Config) -> Vec<String> {
    match PluginRegistry::discover(config) {
        Ok(registry) => registry
            .plugins()
            .map(|plugin| format!("{}  v{}", plugin.id, plugin.version))
            .collect(),
        Err(error) => vec![format!("Plugin scan failed: {error}")],
    }
}

fn search_sources(config: &Config) -> Vec<String> {
    let mut sources = vec![ALL_SOURCES.into(), "soulseek".into()];
    if let Ok(registry) = PluginRegistry::discover(config) {
        sources.extend(
            registry
                .plugins()
                .filter(|plugin| plugin.supports_search)
                .map(|plugin| plugin.id.clone()),
        );
    }
    sources.sort();
    sources.dedup();
    sources
}

fn begin_plugin_login(
    config_path: &str,
    plugin_id: &str,
) -> Result<r1pper::auth::DeviceLogin, String> {
    let config = load_config(config_path)?;
    let registry = PluginRegistry::discover(&config).map_err(|error| error.to_string())?;
    let plugin = registry
        .plugin(plugin_id)
        .ok_or_else(|| format!("plugin `{plugin_id}` was not found"))?;
    let scheme = plugin
        .auth_schemes
        .iter()
        .find(|scheme| scheme.kind() == "oauth_device")
        .ok_or_else(|| format!("plugin `{plugin_id}` has no device-code authentication"))?;
    r1pper::auth::begin_device_login(plugin_id, scheme).map_err(|error| error.to_string())
}

fn save_access_token(config_path: PathBuf, plugin: String, token: String) -> Result<(), String> {
    let scheme = if config_path.exists() {
        Config::from_file(&config_path)
            .map_err(|error| error.to_string())?
            .auth
            .get(&plugin)
            .map(|credentials| credentials.scheme.clone())
            .filter(|scheme| !scheme.is_empty())
            .unwrap_or_else(|| "bearer".into())
    } else {
        "bearer".into()
    };

    Config::save_auth_credentials(
        config_path,
        &plugin,
        &AuthCredentials {
            scheme,
            values: BTreeMap::from([("access_token".into(), token)]),
        },
    )
    .map_err(|error| error.to_string())
}

fn search(
    query: String,
    config_path: String,
    selected_source: String,
) -> Result<SearchOutput, String> {
    let config = load_config(&config_path)?;
    let registry = PluginRegistry::discover(&config).map_err(|error| error.to_string())?;
    let mut sources: BTreeSet<String> = registry
        .plugins()
        .filter(|plugin| plugin.supports_search)
        .map(|plugin| plugin.id.clone())
        .collect();
    sources.insert("soulseek".into());

    if selected_source != ALL_SOURCES {
        sources.retain(|source| source == &selected_source);
    }

    let mut results_output = Vec::new();
    let mut logs = Vec::new();
    for source in sources {
        let results = match source.as_str() {
            "spotify" => r1pper::spotify::search(&config, &query, 10),
            "soulseek" => r1pper::soulseek::search(&config, &query, 10),
            _ => registry.search(&source, &query, 10, &config),
        };
        match results {
            Ok(results) => results_output.extend(results.into_iter().map(|result| {
                ResultItem {
                    source: result.plugin_id,
                    title: non_empty(result.title, "Unknown title"),
                    artist: result
                        .artist
                        .map(|artist| non_empty(artist, "Unknown artist"))
                        .unwrap_or_else(|| "Unknown artist".into()),
                    url: result.url,
                    artwork_url: result.artwork_url,
                }
            })),
            Err(error) => logs.push(format!("Skipped [{source}] {error}")),
        }
    }

    if results_output.is_empty() {
        return Err(if logs.is_empty() {
            "no search sources are available".into()
        } else {
            logs.join("\n")
        });
    }
    Ok(SearchOutput {
        results: results_output,
        logs,
    })
}

fn non_empty(value: String, fallback: &str) -> String {
    let value = value.trim().to_owned();
    (!value.is_empty())
        .then_some(value)
        .unwrap_or_else(|| fallback.into())
}

async fn fetch_artwork(url: String) -> Result<Vec<u8>, String> {
    reqwest::get(url)
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| error.to_string())
}

fn download_selected(
    urls: Vec<String>,
    config_path: String,
    overwrite: bool,
    progress: &Arc<Mutex<String>>,
) -> Result<Vec<String>, String> {
    let mut output = Vec::new();
    for url in urls {
        output.extend(download(url, config_path.clone(), overwrite, progress)?);
    }
    Ok(output)
}

fn save_config(
    path: String,
    output_directory: String,
    concurrent_downloads: String,
    audio_format: AudioFormat,
    embed_artwork: bool,
    normalize_loudness: bool,
) -> Result<(), String> {
    if path.is_empty() {
        return Err("a configuration file path is required".into());
    }
    if output_directory.is_empty() {
        return Err("an output directory is required".into());
    }

    let config_path = PathBuf::from(&path);
    let mut config = if config_path.exists() {
        Config::from_file(&config_path).map_err(|error| error.to_string())?
    } else {
        Config::default()
    };
    config.output_directory = PathBuf::from(output_directory);
    config.concurrent_downloads = concurrent_downloads
        .parse()
        .map_err(|_| "parallel downloads must be a positive whole number".to_string())?;
    audio_format.apply(&mut config);
    config.audio.embed_artwork = embed_artwork;
    config.audio.normalize_loudness = normalize_loudness;
    config.validate().map_err(|error| error.to_string())?;

    let contents = toml::to_string_pretty(&config).map_err(|error| error.to_string())?;
    fs::write(path, contents).map_err(|error| error.to_string())
}

fn download(
    url: String,
    config_path: String,
    overwrite: bool,
    progress: &Arc<Mutex<String>>,
) -> Result<Vec<String>, String> {
    let config = load_config(&config_path)?;
    let downloader = Downloader::new(config).map_err(|error| error.to_string())?;
    let result = downloader
        .download_all_with_progress(
            DownloadRequest {
                url,
                overwrite: overwrite.then_some(OverwritePolicy::Overwrite),
            },
            &mut |event| match event {
                DownloadEvent::Resolving => set_progress(progress, "Resolving source...".into()),
                DownloadEvent::StartingItem {
                    index,
                    total,
                    title,
                } => set_progress(
                    progress,
                    format!(
                        "Downloading {index}/{total}: {}",
                        title.unwrap_or_else(|| "untitled".into())
                    ),
                ),
                DownloadEvent::Downloading { downloaded, total } => set_progress(
                    progress,
                    total
                        .map(|total| format!("Downloading {downloaded}/{total} bytes"))
                        .unwrap_or_else(|| format!("Downloading {downloaded} bytes")),
                ),
                DownloadEvent::Processing => set_progress(progress, "Processing audio...".into()),
                DownloadEvent::Finished(path) => {
                    set_progress(progress, format!("Saved {}", path.display()))
                }
                DownloadEvent::Skipped(path) => {
                    set_progress(progress, format!("Skipped {}", path.display()))
                }
            },
        )
        .map_err(|error| error.to_string())?;

    Ok(result
        .downloads
        .into_iter()
        .map(|download| {
            format!(
                "{} {}",
                if download.skipped { "Skipped" } else { "Saved" },
                download.output_path.display()
            )
        })
        .collect())
}

fn set_progress(progress: &Arc<Mutex<String>>, value: String) {
    if let Ok(mut current) = progress.lock() {
        *current = value;
    }
}
