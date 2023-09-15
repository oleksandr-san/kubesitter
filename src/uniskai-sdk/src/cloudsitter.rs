use crate::{Identification, Result, UniskaiClient};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct CloudsitterSchedule {
    pub hours: Vec<bool>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct CloudsitterResource {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub identification: Identification,
    pub pause_from: Option<DateTime<Utc>>,
    pub pause_to: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct CloudsitterPolicy {
    pub id: i64,
    pub name: String,
    pub schedules: CloudsitterSchedule,
    pub resources: Vec<CloudsitterResource>,
    pub disabled: bool,
    pub timezone: String,
    pub gmt: String,
    pub brake_reason: Option<String>,
    pub broken: bool,
    pub next_state: bool,
    pub emails: Option<Vec<String>>,
}

impl UniskaiClient {
    pub async fn list_cloudsitter_policies(&self) -> Result<Vec<CloudsitterPolicy>> {
        let response = self
            .client
            .get(format!(
                "{}/environments/{}/cloudsitter/policies",
                self.base_url(),
                self.env_id
            ))
            .timeout(self.timeout())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        let response = response.error_for_status()?;
        let policies = response.json::<Vec<CloudsitterPolicy>>().await?;
        Ok(policies)
    }
}
