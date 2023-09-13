use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use controller_core::Error;
use uniskai_sdk::{cloudsitter::CloudsitterPolicy, NAMESPACE_TYPES};

pub static POLICY_FINALIZER: &str = "schedulepolicies.api.profisealabs.com";

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
pub enum RequirementOperator {
    #[serde(rename = "in", alias = "In")]
    In,
    #[serde(rename = "notin", alias = "NotIn", alias = "notIn")]
    NotIn,
    #[serde(rename = "exists", alias = "Exists")]
    Exists,
    #[serde(rename = "doesnotexist", alias = "DoesNotExist", alias = "doesNotExist")]
    DoesNotExist,
}

impl ToString for RequirementOperator {
    fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
pub struct LabelSelectorRequirement {
    pub key: String,
    pub operator: RequirementOperator,
    pub values: Option<Vec<String>>,
}

impl LabelSelectorRequirement {
    pub fn to_label_selector(&self) -> String {
        let mut selector = String::new();
        selector.push_str(&self.key);
        selector.push(' ');
        selector.push_str(&self.operator.to_string().to_ascii_lowercase());

        match self.operator {
            RequirementOperator::Exists | RequirementOperator::DoesNotExist => {}
            _ => {
                if let Some(values) = &self.values {
                    selector.push_str(" (");
                    selector.push_str(&values.join(","));
                    selector.push(')');
                }
            }
        }

        selector
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum NamespaceSelector {
    /// MatchNames is a list of namespace name regex patterns.
    /// The requirements are ORed.
    MatchNames(Vec<String>),

    /// MatchLabels is a map of {key,value} pairs. A single {key,value} in the matchLabels map is equivalent to an element of matchExpressions,
    /// whose key field is "key", the operator is "In", and the values array contains only "value".
    /// The requirements are ANDed.
    MatchLabels(BTreeMap<String, String>),

    /// MatchExpressions is a list of label selector requirements.
    /// The requirements are ANDed.
    MatchExpressions(Vec<LabelSelectorRequirement>),
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkTime {
    pub start: chrono::NaiveTime,
    pub stop: chrono::NaiveTime,
    pub days: Vec<chrono::Weekday>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Schedule {
    WorkTimes(Vec<WorkTime>),
}

/// Generate the Kubernetes wrapper struct `SchedulePolicy` from our Spec and Status struct
///
/// This provides a hook for generating the CRD yaml (in crdgen.rs)
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[kube(
    kind = "SchedulePolicy",
    group = "api.profisealabs.com",
    version = "v1alpha",
    namespaced
)]
#[kube(status = "SchedulePolicyStatus", shortname = "schedule")]
#[serde(rename_all = "camelCase")]
pub struct SchedulePolicySpec {
    pub title: String,
    pub suspend: bool,
    pub namespace_selector: NamespaceSelector,
    pub schedule: Schedule,
    pub time_zone: Option<String>,
}

/// The status object of `SchedulePolicy`
#[derive(Deserialize, Serialize, Clone, Default, Debug, JsonSchema)]
pub struct SchedulePolicyStatus {
    pub suspended: bool,
}

impl SchedulePolicy {
    pub fn was_suspended(&self) -> bool {
        self.status.as_ref().map_or(false, |s| s.suspended)
    }
}

fn convert_to_work_times(periods: &Vec<bool>) -> Result<Vec<WorkTime>, Error> {
    let (period_duration, day_length) = match periods.len() {
        168 => (chrono::Duration::hours(1), 24),
        336 => (chrono::Duration::minutes(30), 48),
        672 => (chrono::Duration::minutes(15), 96),
        _ => {
            return Err(Error::InvalidParameters(
                "Unexpected amount of periods in schedule".into(),
            ))
        }
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

    'outer: for (start, stop) in &ranges {
        let day = u8::try_from(*start / day_length).expect("Failed to convert index to u8");
        let day = chrono::Weekday::try_from(day).expect("Failed to convert index to weekday");

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

impl TryFrom<&CloudsitterPolicy> for SchedulePolicySpec {
    type Error = Error;

    fn try_from(policy: &CloudsitterPolicy) -> Result<Self, Self::Error> {
        let work_times = convert_to_work_times(&policy.schedules.hours)?;
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

        let spec = SchedulePolicySpec {
            title: policy.name.clone(),
            suspend: policy.disabled,
            time_zone: Some(policy.timezone.clone()),
            namespace_selector: NamespaceSelector::MatchNames(namespace_names),
            schedule,
        };
        Ok(spec)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, Weekday};

    #[test]
    fn parses_work_time() {
        let schedule = r#"{
            "workTimes": [{
                "start": "08:00:00",
                "stop": "17:00:00",
                "days": ["Mon", "Tue", "Wed", "Thu", "Fri"]
            }]
        }
        "#;
        let schedule: super::Schedule = serde_json::from_str(schedule).unwrap();
        match schedule {
            super::Schedule::WorkTimes(times) => {
                assert_eq!(times.len(), 1);
                let super::WorkTime { start, stop, days } = &times[0];
                assert_eq!(*start, NaiveTime::parse_from_str("8:00", "%H:%M").unwrap());
                assert_eq!(*stop, NaiveTime::parse_from_str("17:00", "%H:%M").unwrap());
                assert_eq!(
                    *days,
                    vec![
                        Weekday::Mon,
                        Weekday::Tue,
                        Weekday::Wed,
                        Weekday::Thu,
                        Weekday::Fri
                    ]
                );
            }
        }
    }

    #[test]
    fn parses_label_expressions() {
        let selector = r#"[
            {
                "key": "label1",
                "operator": "in",
                "values": ["value1", "value2"]
            },
            {
                "key": "label2",
                "operator": "NotIn",
                "values": ["value3", "value4"]
            },
            {
                "key": "label3",
                "operator": "Exists"
            }
        ]"#;
        let selector: Vec<super::LabelSelectorRequirement> = serde_json::from_str(selector).unwrap();
        assert_eq!(selector.len(), 3);

        let selector = &selector[0];
        assert_eq!(selector.key, "label1");
        assert_eq!(selector.operator, super::RequirementOperator::In);
        assert_eq!(
            selector.values,
            Some(vec!["value1".to_string(), "value2".to_string()])
        );
    }
}
