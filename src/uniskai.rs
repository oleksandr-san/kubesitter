use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use thiserror::Error;

use controller::{
    controller::{NamespaceSelector, SchedulePolicySpec},
    schedule::Schedule,
    WorkTime,
};

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CloudsitterSchedule {
    pub hours: Vec<bool>,
}

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
    pub brake_reason: Option<String>,
    pub broken: bool,
    pub disabled: bool,
    pub emails: Option<Vec<String>>,
    pub schedules: CloudsitterSchedule,
    pub resources: Vec<CloudsitterResource>,
    pub next_state: bool,
    pub timezone: String,
    pub gmt: String,

    pub id: i64,
    pub name: String,
}

static NAMESPACE_TYPES: [&str; 2] = ["kubernetes-namespace", "eks-namespace"];

#[derive(Debug, Error)]
pub enum Error {
    #[error("Invalid schedule value")]
    InvalidSchedule,

    #[error("Out of range")]
    OutOfRange(#[from] chrono::OutOfRange),
}

fn convert_to_work_times(periods: &Vec<bool>) -> Result<Vec<WorkTime>, Error> {
    let (period_duration, day_length) = match periods.len() {
        168 => (chrono::Duration::hours(1), 24),
        336 => (chrono::Duration::minutes(30), 48),
        672 => (chrono::Duration::minutes(15), 96),
        _ => return Err(Error::InvalidSchedule),
    };

    let mut ranges = Vec::new();
    let mut current_range: Option<(usize, usize)> = None;

    for (i, state) in periods.iter().enumerate() {
        if i % day_length == 0 {
            if let Some(range) = current_range {
                ranges.push(range);
                current_range = None;
            }
        }

        if *state {
            if let Some((start, _)) = current_range {
                current_range = Some((start, i));
            } else {
                current_range = Some((i, i));
            }
        } else if let Some(range) = current_range {
            ranges.push(range);
            current_range = None;
        }
    }

    if let Some(range) = current_range {
        ranges.push(range);
    }

    let mut work_times: Vec<WorkTime> = Vec::new();

    'outer: for (start, stop) in ranges.iter() {
        let day =
            chrono::Weekday::try_from(u8::try_from(*start / day_length).expect("Failed to convert day"))?;

        let start = chrono::NaiveTime::MIN
            + period_duration * i32::try_from(*start % day_length).expect("Failed to convert start");
        let stop = if (stop + 1) % day_length == 0 {
            chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap()
        } else {
            chrono::NaiveTime::MIN
                + period_duration * i32::try_from((stop + 1) % day_length).expect("Failed to convert stop")
        };

        for work_time in &mut work_times {
            if work_time.start == start && work_time.stop == stop {
                work_time.days.push(day);
                continue 'outer;
            }
        }

        work_times.push(WorkTime {
            days: vec![day],
            start,
            stop,
        });
    }

    Ok(work_times)
}

fn convert_to_schedule_policy(policy: &CloudsitterPolicy) -> SchedulePolicySpec {
    let work_times = convert_to_work_times(&policy.schedules.hours).expect("Failed to convert schedule");
    let schedule = Schedule::WorkTimes(work_times);

    let mut namespace_names = Vec::new();
    for resource in &policy.resources {
        if NAMESPACE_TYPES.contains(&resource.ty.as_str()) {
            if let Some(name) = resource.identification.name() {
                namespace_names.push(name.to_string());
            }

            // TODO: handle pause_from and pause_to
        }
    }

    SchedulePolicySpec {
        title: policy.name.clone(),
        suspend: policy.disabled,
        time_zone: Some(policy.timezone.clone()),
        namespace_selector: NamespaceSelector::MatchNames(namespace_names),
        schedule,
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args();
    args.next();

    let command = args.next().expect("No command provided");
    if command == "query" {
        let api_key = args.next().expect("No API key provided");
        let env_id = args.next().expect("No environment ID provided");

        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "https://feature-environments.profisealabs.com/api/environments/{}/cloudsitter/policies",
                env_id
            ))
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
            .unwrap();

        let body = response.text().await.unwrap();
        println!("{}", body);

        // let policies: Vec<CloudsitterPolicy> = serde_json::from_str(&body).unwrap();
        // for policy in policies {
        //     println!("Policy: {:?}", policy);
        // }
    } else if command == "parse" {
        let file = args.next().expect("No file path provided");
        let file = std::fs::File::open(file).expect("Failed to open file");
        println!("File opened");
        // let file = std::io::BufReader::new(file);
        // let text = file.lines().collect::<Result<Vec<String>, _>>().expect("Failed to read file");
        // let text = text.join("\n");
        // print!("File read, text: {}", text);

        let policies: Vec<CloudsitterPolicy> = serde_json::from_reader(file).expect("Failed to parse JSON");
        for policy in policies {
            let policy = convert_to_schedule_policy(&policy);
            println!("Policy: {:?}", policy);
        }
    } else {
        panic!("Unknown command: {}", command);
    }
}
