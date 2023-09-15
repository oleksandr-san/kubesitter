pub mod cloudsitter;
pub use cloudsitter::*;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub static DEFAULT_API_URL : &str = "profisealabs.com";
pub static API_KEY_ENV_VAR: &str = "UNISKAI_API_KEY";
pub static API_URL_ENV_VAR: &str = "UNISKAI_API_URL";
pub static ENV_ID_ENV_VAR: &str = "UNISKAI_ENV_ID";
pub static NAMESPACE_TYPES: [&str; 2] = ["kubernetes-namespace", "eks-namespace"];

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "provider")]
pub enum Identification {
    #[serde(rename = "aws")]
    AwsIdentification { arn: String, region: String },
    #[serde(rename = "azure")]
    AzureIdentification { rn: String, region: String },
    #[serde(rename = "gcp")]
    GcpIdentification { grn: String, region: String },
}

impl Identification {
    pub fn name(&self) -> Option<&str> {
        fn parse_id(id: &str) -> Option<&str> {
            if let Some((_, id)) = id.rsplit_once('/') {
                Some(id)
            } else {
                None
            }
        }
        match self {
            Identification::AwsIdentification { arn, .. } => parse_id(arn),
            Identification::AzureIdentification { rn, .. } => parse_id(rn),
            Identification::GcpIdentification { grn, .. } => parse_id(grn),
        }
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    EnvVar(#[from] std::env::VarError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub struct UniskaiClient {
    api_key: String,
    api_url: String,
    env_id: String,
    client: reqwest::Client,
}

impl UniskaiClient {
    pub fn try_default() -> Result<Self>  {
        let api_key = std::env::var(API_KEY_ENV_VAR)?;
        let env_id = std::env::var(ENV_ID_ENV_VAR)?;
        let api_url = std::env::var(API_URL_ENV_VAR).unwrap_or_else(|_| DEFAULT_API_URL.to_string());
        Ok(Self::new(api_key, api_url, env_id))
    }

    pub fn new(api_key: String, api_url: String, env_id: String) -> Self {
        Self {
            api_key,
            api_url: format!("https://{}/api", api_url),
            env_id,
            client: reqwest::Client::new(),
        }
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.api_url
    }

    pub(crate) fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(20)
    }
}
