use crate::Identification;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CloudsitterSchedule {
    pub hours: Vec<bool>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CloudsitterResource {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub identification: Identification,
    pub pause_from: Option<DateTime<Utc>>,
    pub pause_to: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
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
