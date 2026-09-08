use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clap::{Parser, Subcommand};
use console::style;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use r1pper::{
    Config, DownloadEvent, DownloadRequest, Downloader, Error, Result, auth::AuthScheme,
    plugin::PluginRegistry,
};
use serde::Deserialize;
use url::Url;

#[derive(Debug, Parser)]
#[command(version, about = "Modular audio downloader")]
struct Cli {
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download audio from a source URL.
    Download {
        url: String,
        /// Replace an existing output file instead of skipping or renaming it.
        #[arg(long)]
        force: bool,
    },
    /// Search installed source plugins for downloadable media.
    Search {
        query: String,
        /// Restrict search to one or more plugins. Repeat to search several.
        #[arg(short, long = "plugin")]
        plugins: Vec<String>,
        /// Maximum results from each selected plugin.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    /// Inspect configured source plugins.
    Plugins {
        #[command(subcommand)]
        command: PluginCommand,
    },
    /// Print the effective configuration as TOML.
    Config {
        /// Include authentication credential values in the output.
        #[arg(long)]
        show_secrets: bool,
    },
    /// Show the authentication flow declared by an installed plugin.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    List,
    Inspect { id: String },
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Start an authentication flow for a plugin.
    Login { id: String },
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    interval: Option<u64>,
    expires_in: u64,
}

#[derive(Deserialize)]
struct DeviceTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("{} {error}", style("error:").red().bold());
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let config_path = cli.config.unwrap_or_else(|| PathBuf::from("r1pper.toml"));
    let config = if config_path.exists() {
        Config::from_file(&config_path)?
    } else {
        Config::default()
    };

    match cli.command {
        Command::Config { show_secrets } => {
            let mut displayed = config.clone();
            if !show_secrets {
                for credentials in displayed.auth.values_mut() {
                    for value in credentials.values.values_mut() {
                        *value = "<redacted>".into();
                    }
                }
            }
            println!(
                "{}",
                toml::to_string_pretty(&displayed)
                    .map_err(|error| Error::Message(error.to_string()))?
            );
            Ok(())
        }
        Command::Plugins { command } => {
            let registry = PluginRegistry::discover(&config)?;
            match command {
                PluginCommand::List => {
                    for plugin in registry.plugins() {
                        println!(
                            "{} {}  {}  {}",
                            style("plugin").cyan().bold(),
                            style(&plugin.id).white().bold(),
                            style(format!("v{}", plugin.version)).dim(),
                            style(plugin.url_patterns.join(", ")).yellow(),
                        );
                    }
                    Ok(())
                }
                PluginCommand::Inspect { id } => {
                    let plugin = registry
                        .plugin(&id)
                        .ok_or_else(|| Error::Message(format!("plugin {id} is not installed")))?;
                    println!(
                        "{} {}",
                        style("Plugin").cyan().bold(),
                        style(&plugin.id).white().bold()
                    );
                    println!("  {} {}", style("Version").dim(), plugin.version);
                    println!(
                        "  {} {}",
                        style("Manifest").dim(),
                        plugin.manifest_path.display()
                    );
                    println!(
                        "  {} {}",
                        style("URLs").dim(),
                        plugin.url_patterns.join(", ")
                    );
                    if plugin.auth_schemes.is_empty() {
                        println!("  {} {}", style("Auth").dim(), style("none").yellow());
                    } else {
                        for scheme in &plugin.auth_schemes {
                            println!(
                                "  {} {} {}{}",
                                style("Auth").dim(),
                                style(scheme.id()).magenta().bold(),
                                style(scheme.kind()).green(),
                                scheme
                                    .description()
                                    .map(|description| format!(" ({description})"))
                                    .unwrap_or_default()
                            );
                        }
                    }
                    Ok(())
                }
            }
        }
        Command::Download { url, force } => {
            let progress = ProgressBar::new_spinner();
            progress.set_style(
                ProgressStyle::with_template("{spinner:.yellow} {msg:.bold}")
                    .expect("progress template is valid"),
            );
            progress.enable_steady_tick(Duration::from_millis(100));
            let downloader = Downloader::new(config)?;
            let mut current_track = String::new();
            let result = downloader.download_all_with_progress(
                DownloadRequest {
                    url,
                    overwrite: force.then_some(r1pper::OverwritePolicy::Overwrite),
                },
                &mut |event| match event {
                    DownloadEvent::Resolving => progress.set_message("Resolving source..."),
                    DownloadEvent::StartingItem {
                        index,
                        total,
                        title,
                    } => {
                        progress.set_style(
                            ProgressStyle::with_template("{spinner:.magenta} {msg:.bold}")
                                .expect("progress template is valid"),
                        );
                        progress.set_position(0);
                        progress.set_length(0);
                        current_track = format!(
                            "Track {index}/{total}: {}",
                            title.unwrap_or_else(|| "Unknown title".into())
                        );
                        progress.set_message(current_track.clone());
                    }
                    DownloadEvent::Downloading { downloaded, total } => {
                        if let Some(total) = total {
                            progress.set_length(total);
                            progress.set_style(
                                ProgressStyle::with_template(
                                    "{bar:40.green/blue} {bytes:>8}/{total_bytes:>8} {msg:.cyan}",
                                )
                                .expect("progress template is valid")
                                .progress_chars("=> "),
                            );
                        }
                        progress.set_position(downloaded);
                        progress.set_message(format!(
                            "{}  {}",
                            current_track,
                            HumanBytes(downloaded)
                        ));
                    }
                    DownloadEvent::Processing => {
                        progress.set_style(
                            ProgressStyle::with_template("{spinner:.blue} {msg:.bold}")
                                .expect("progress template is valid"),
                        );
                        progress
                            .set_message(format!("{}  Transcoding and tagging...", current_track));
                    }
                    DownloadEvent::Finished(path) => {
                        progress.set_message(format!("Saved {}", path.display()));
                    }
                    DownloadEvent::Skipped(path) => {
                        progress.set_message(format!("Skipped {}", path.display()));
                    }
                },
            )?;
            progress.finish_and_clear();
            for download in result.downloads {
                if download.skipped {
                    println!(
                        "{} {}",
                        style("skipped").yellow().bold(),
                        download.output_path.display()
                    );
                } else {
                    println!(
                        "{} {}",
                        style("saved").green().bold(),
                        download.output_path.display()
                    );
                }
            }
            Ok(())
        }
        Command::Search {
            query,
            plugins,
            limit,
        } => {
            let registry = PluginRegistry::discover(&config)?;
            let plugins = if plugins.is_empty() {
                vec!["spotify".into(), "soulseek".into()]
            } else {
                plugins
            };
            for plugin in plugins {
                let results = match plugin.as_str() {
                    "spotify" => r1pper::spotify::search(&config, &query, limit)?,
                    "soulseek" => r1pper::soulseek::search(&config, &query, limit)?,
                    _ => registry.search(&plugin, &query, limit, &config)?,
                };
                for result in results {
                    let details = [result.artist.as_deref(), result.album.as_deref()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" - ");
                    let size = result
                        .size
                        .map(HumanBytes)
                        .map(|size| format!("  {size}"))
                        .unwrap_or_default();
                    println!(
                        "{}  {}{}\n  {}",
                        style(result.plugin_id).magenta().bold(),
                        result.title,
                        if details.is_empty() {
                            String::new()
                        } else {
                            format!("  {details}")
                        },
                        result.url,
                    );
                    if !size.is_empty() {
                        println!("  {size}");
                    }
                }
            }
            Ok(())
        }
        Command::Auth {
            command: AuthCommand::Login { id },
        } => {
            if id == "spotify" {
                return r1pper::spotify::login(&config_path);
            }
            let registry = PluginRegistry::discover(&config)?;
            let plugin = registry
                .plugin(&id)
                .ok_or_else(|| Error::Message(format!("plugin {id} is not installed")))?;
            if let Some(scheme @ AuthScheme::OAuthDevice { .. }) = plugin
                .auth_schemes
                .iter()
                .find(|scheme| matches!(scheme, AuthScheme::OAuthDevice { .. }))
            {
                return run_device_flow(&config_path, &plugin.id, scheme);
            }
            let oauth = plugin
                .auth_schemes
                .iter()
                .find_map(|scheme| match scheme {
                    AuthScheme::OAuth { .. } => Some(scheme),
                    _ => None,
                })
                .ok_or_else(|| Error::Message(format!("plugin {id} does not declare OAuth")))?;
            let AuthScheme::OAuth {
                authorization_url,
                client_id,
                scopes,
                redirect_uri,
                ..
            } = oauth
            else {
                unreachable!("the OAuth scheme was selected above")
            };
            let mut authorization_url = Url::parse(authorization_url)
                .map_err(|error| Error::Message(format!("invalid OAuth URL: {error}")))?;
            authorization_url
                .query_pairs_mut()
                .append_pair("response_type", "code")
                .append_pair("client_id", client_id)
                .append_pair("redirect_uri", redirect_uri);
            if !scopes.is_empty() {
                authorization_url
                    .query_pairs_mut()
                    .append_pair("scope", &scopes.join(" "));
            }
            println!("Open this URL to authorize {}:", plugin.id);
            println!("{authorization_url}");
            println!(
                "After authorization, exchange the returned code at the plugin's token endpoint and store the resulting credentials under [auth.{}].",
                plugin.id
            );
            Ok(())
        }
    }
}

fn run_device_flow(
    config_path: &std::path::Path,
    plugin_id: &str,
    scheme: &AuthScheme,
) -> Result<()> {
    let AuthScheme::OAuthDevice {
        device_code_url,
        token_url,
        client_id,
        client_secret,
        device_name,
        ..
    } = scheme
    else {
        unreachable!("device flow is invoked only for OAuthDevice")
    };
    let client = reqwest::blocking::Client::new();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::Message(error.to_string()))?
        .as_nanos();
    let device_id = format!("r1pper-{:x}-{timestamp:x}", std::process::id());
    let response = client
        .post(device_code_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_body(&[
            ("client_id", client_id),
            ("device_id", &device_id),
            ("device_name", device_name),
        ]))
        .send()?
        .error_for_status()?;
    let device: DeviceCodeResponse = response.json()?;
    println!(
        "Open {} and enter code: {}",
        device.verification_url, device.user_code
    );

    let deadline = Instant::now() + Duration::from_secs(device.expires_in);
    let interval = Duration::from_secs(device.interval.unwrap_or(5).max(1));
    while Instant::now() < deadline {
        thread::sleep(interval);
        let response = client
            .post(token_url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body(&[
                ("grant_type", "device_code"),
                ("code", &device.device_code),
                ("client_id", client_id),
                ("client_secret", client_secret),
            ]))
            .send()?;
        let token: DeviceTokenResponse = response.json()?;
        if let Some(token) = token.access_token {
            Config::save_auth_credentials(
                config_path,
                plugin_id,
                &r1pper::auth::AuthCredentials {
                    scheme: scheme.id().into(),
                    values: [("access_token".into(), token)].into_iter().collect(),
                },
            )?;
            println!(
                "Authorization complete. Credentials saved to {}.",
                config_path.display()
            );
            return Ok(());
        }
        if token.error.as_deref() != Some("authorization_pending") {
            return Err(Error::Message(
                token
                    .error_description
                    .or(token.error)
                    .unwrap_or_else(|| "OAuth device authorization failed".into()),
            ));
        }
    }
    Err(Error::Message(
        "OAuth device code expired before authorization completed".into(),
    ))
}

fn form_body(values: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in values {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}
