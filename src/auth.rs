use std::{
    collections::BTreeMap,
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{Config, Error, Result};

/// Credentials supplied to a single plugin. Plugin authors define the expected keys for each scheme.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AuthCredentials {
    #[serde(default)]
    pub scheme: String,
    #[serde(flatten)]
    pub values: BTreeMap<String, String>,
}

/// An authentication mechanism declared by a source plugin.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthScheme {
    Bearer {
        id: String,
        description: Option<String>,
    },
    ApiKey {
        id: String,
        header: String,
        description: Option<String>,
    },
    #[serde(rename = "oauth")]
    OAuth {
        id: String,
        authorization_url: String,
        token_url: String,
        client_id: String,
        #[serde(default)]
        scopes: Vec<String>,
        redirect_uri: String,
    },
    #[serde(rename = "oauth_device")]
    OAuthDevice {
        id: String,
        device_code_url: String,
        token_url: String,
        client_id: String,
        client_secret: String,
        device_name: String,
    },
}

impl AuthScheme {
    pub fn id(&self) -> &str {
        match self {
            Self::Bearer { id, .. }
            | Self::ApiKey { id, .. }
            | Self::OAuth { id, .. }
            | Self::OAuthDevice { id, .. } => id,
        }
    }

    pub fn kind(&self) -> &str {
        match self {
            Self::Bearer { .. } => "bearer",
            Self::ApiKey { .. } => "api_key",
            Self::OAuth { .. } => "oauth",
            Self::OAuthDevice { .. } => "oauth_device",
        }
    }

    pub fn description(&self) -> Option<&str> {
        match self {
            Self::Bearer { description, .. } | Self::ApiKey { description, .. } => {
                description.as_deref()
            }
            Self::OAuth { .. } => Some("OAuth authorization-code flow"),
            Self::OAuthDevice { .. } => Some("OAuth device authorization flow"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceLogin {
    pub plugin_id: String,
    pub scheme: String,
    pub user_code: String,
    pub verification_url: String,
    device_code: String,
    token_url: String,
    client_id: String,
    client_secret: String,
    interval: Duration,
    expires_at: Instant,
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

/// Begins an OAuth device authorization declared by a plugin manifest.
pub fn begin_device_login(plugin_id: &str, scheme: &AuthScheme) -> Result<DeviceLogin> {
    let AuthScheme::OAuthDevice {
        device_code_url,
        token_url,
        client_id,
        client_secret,
        device_name,
        ..
    } = scheme
    else {
        return Err(Error::InvalidConfig("plugin does not support device authorization".into()));
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::Message(error.to_string()))?
        .as_nanos();
    let device_id = format!("r1pper-{:x}-{timestamp:x}", std::process::id());
    let response: DeviceCodeResponse = reqwest::blocking::Client::new()
        .post(device_code_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_body(&[
            ("client_id", client_id.as_str()),
            ("device_id", device_id.as_str()),
            ("device_name", device_name.as_str()),
        ]))
        .send()?
        .error_for_status()?
        .json()?;

    Ok(DeviceLogin {
        plugin_id: plugin_id.into(),
        scheme: scheme.id().into(),
        user_code: response.user_code,
        verification_url: response.verification_url,
        device_code: response.device_code,
        token_url: token_url.clone(),
        client_id: client_id.clone(),
        client_secret: client_secret.clone(),
        interval: Duration::from_secs(response.interval.unwrap_or(5).max(1)),
        expires_at: Instant::now() + Duration::from_secs(response.expires_in),
    })
}

/// Waits for authorization of a pending plugin device code and persists its token.
pub fn finish_device_login(login: DeviceLogin, config_path: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    while Instant::now() < login.expires_at {
        thread::sleep(login.interval);
        let token: DeviceTokenResponse = client
            .post(&login.token_url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body(&[
                ("grant_type", "device_code"),
                ("code", login.device_code.as_str()),
                ("client_id", login.client_id.as_str()),
                ("client_secret", login.client_secret.as_str()),
            ]))
            .send()?
            .json()?;
        if let Some(access_token) = token.access_token {
            return Config::save_auth_credentials(
                config_path,
                &login.plugin_id,
                &AuthCredentials {
                    scheme: login.scheme,
                    values: BTreeMap::from([("access_token".into(), access_token)]),
                },
            );
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
    Err(Error::Message("OAuth device code expired before authorization completed".into()))
}

fn form_body(values: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in values {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}
