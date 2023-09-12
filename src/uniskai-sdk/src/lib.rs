pub mod cloudsitter;
pub use cloudsitter::*;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub struct UniskaiClient {
    api_key: String,
    env_id: String,
    client: reqwest::Client,
    feature_env: Option<String>,
}

impl UniskaiClient {
    pub fn new(api_key: String, env_id: String) -> Self {
        Self {
            api_key,
            env_id,
            client: reqwest::Client::new(),
            feature_env: None,
        }
    }

    pub fn with_feature_env(mut self, feature_env: String) -> Self {
        self.feature_env = Some(feature_env);
        self
    }

    pub fn base_url(&self) -> String {
        if let Some(feature_env) = &self.feature_env {
            format!("https://{}.profisealabs.com/api", feature_env)
        } else {
            "https://profisealabs.com/api".to_string()
        }
    }
}
