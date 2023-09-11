pub mod cloudsitter;

use serde::{Deserialize, Serialize};

pub static NAMESPACE_TYPES: [&str; 2] = ["kubernetes-namespace", "eks-namespace"];

#[derive(Deserialize, Serialize, Clone, Debug)]
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
