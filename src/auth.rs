use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
