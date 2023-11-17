use controller_core::Error;
use uniskai_sdk::{cloudsitter::CloudsitterPolicy, NAMESPACE_TYPES};

use k8s_openapi::{api::core::v1::Namespace, Resource};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const TIME_PATTERN: &str = r"^([01]\d|2[0-3]):[0-5]\d:[0-5]\d$";
const DATE_TIME_PATTERN: &str = r"^\d{4}-\d{2}-\d{2}T([01]\d|2[0-3]):[0-5]\d:[0-5]\d$";
const DATE_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";
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

impl RequirementOperator {
    fn as_str(&self) -> &str {
        match self {
            RequirementOperator::In => "in",
            RequirementOperator::NotIn => "notin",
            RequirementOperator::Exists => "exists",
            RequirementOperator::DoesNotExist => "doesnotexist",
        }
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

        match self.operator {
            RequirementOperator::Exists => {
                selector.push_str(&self.key);
            },
            RequirementOperator::DoesNotExist => {
                selector.push('!');
                selector.push_str(&self.key);
            },
            RequirementOperator::In | RequirementOperator::NotIn => {
                selector.push_str(&self.key);
                selector.push(' ');
                selector.push_str(self.operator.as_str());
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

#[derive(Deserialize, Serialize, Copy, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AssignmentType {
    Skip,
    Work,
    Sleep,
}

fn v1() -> String {
    Namespace::API_VERSION.to_string()
}

fn namespace() -> String {
    Namespace::KIND.to_string()
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceReference {
    #[serde(default = "v1")]
    pub api_version: String,
    #[serde(default = "namespace")]
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ResourceFilter {
    MatchResources(Vec<ResourceReference>),
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    #[serde(rename = "type")]
    pub ty: AssignmentType,
    #[schemars(regex = "DATE_TIME_PATTERN")]
    #[serde(default)]
    #[serde(deserialize_with = "safe_date_time::deserialize")]
    pub from: Option<chrono::NaiveDateTime>,
    #[serde(default)]
    #[schemars(regex = "DATE_TIME_PATTERN")]
    #[serde(deserialize_with = "safe_date_time::deserialize")]
    pub to: Option<chrono::NaiveDateTime>,
    pub resource_filter: Option<ResourceFilter>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkTime {
    #[schemars(regex = "TIME_PATTERN")]
    pub start: chrono::NaiveTime,
    #[schemars(regex = "TIME_PATTERN")]
    pub stop: chrono::NaiveTime,
    pub days: Vec<chrono::Weekday>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Schedule {
    WorkTimes(Vec<WorkTime>),
}

/// `SchedulePoicy` allows to define a schedule for a set of namespaces.
/// The schedule is defined by a set of `WorkTime` objects.
/// The `SchedulePolicy` object is applied to namespaces that match the `NamespaceSelector`.
/// The `SchedulePolicy` object can be suspended by setting the `suspend` field to `true`.
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
    pub assignments: Option<Vec<Assignment>>,
    pub time_zone: Option<String>,
}

/// The status object of `SchedulePolicy`
#[derive(Deserialize, Serialize, Clone, Default, Debug, JsonSchema)]
pub struct SchedulePolicyStatus {
    pub suspended: bool,
}

impl Assignment {
    pub fn is_active_at(&self, timestamp: &chrono::NaiveDateTime) -> bool {
        let from_condition = match &self.from {
            Some(from_time) => timestamp >= from_time,
            None => true,
        };
        let to_condition = match &self.to {
            Some(to_time) => timestamp <= to_time,
            None => true,
        };
        from_condition && to_condition
    }
}

impl ResourceReference {
    pub fn new_namespace(name: &str) -> Self {
        Self {
            api_version: v1(),
            kind: namespace(),
            name: name.to_string(),
            namespace: None,
        }
    }

    pub fn matches(&self, other: &ResourceReference) -> bool {
        self.kind.eq_ignore_ascii_case(&other.kind)
            && self.api_version.eq_ignore_ascii_case(&other.api_version)
            && self.name == other.name
            && self.namespace == other.namespace
    }
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

fn parse_fixed_offset(offset: &str) -> Option<chrono::FixedOffset> {
    if offset.eq_ignore_ascii_case("Z")
        || offset.eq_ignore_ascii_case("UTC")
        || offset.eq_ignore_ascii_case("GMT")
    {
        return chrono::FixedOffset::east_opt(0);
    }
    let offset = if offset.starts_with("UTC") || offset.starts_with("GMT") {
        &offset[3..]
    } else {
        offset
    };

    // Extract the signless part of the offset
    let is_negative = offset.starts_with('-');
    let offset = if is_negative || offset.starts_with('+') {
        &offset[1..]
    } else {
        offset
    };

    // Split the signless offset into hours and minutes
    let mut iter = offset.split(':');
    let hours: i32 = iter.next()?.parse().ok()?;
    let minutes: i32 = iter.next().unwrap_or("0").parse().ok()?;

    // Convert the parsed hours and minutes into seconds
    let mut total_seconds = hours * 3600 + minutes * 60;
    if is_negative {
        total_seconds = -total_seconds;
    }

    chrono::FixedOffset::east_opt(total_seconds)
}

pub fn convert_to_local_time<Tz: chrono::TimeZone>(
    time: &chrono::DateTime<Tz>,
    tz: Option<&String>,
) -> Result<chrono::NaiveDateTime, Error> {
    if let Some(tz) = tz {
        match tz.parse::<chrono_tz::Tz>() {
            Ok(tz) => Ok(time.with_timezone(&tz).naive_local()),
            Err(err) => {
                if let Some(offset) = parse_fixed_offset(tz) {
                    Ok(time.with_timezone(&offset).naive_local())
                } else {
                    Err(Error::InvalidParameters(err.into()))
                }
            }
        }
    } else {
        Ok(time.naive_utc())
    }
}

impl TryFrom<&CloudsitterPolicy> for SchedulePolicySpec {
    type Error = Error;

    fn try_from(policy: &CloudsitterPolicy) -> Result<Self, Self::Error> {
        let work_times = convert_to_work_times(&policy.schedules.hours)?;
        let schedule = Schedule::WorkTimes(work_times);
        let mut assignments = Vec::new();
        let mut namespace_names = Vec::new();
        let time_zone = Some(policy.timezone.clone());

        for resource in &policy.resources {
            let Some(name) = resource.identification.name() else { continue };
            if NAMESPACE_TYPES.contains(&resource.ty.as_str()) {
                namespace_names.push(name.to_string());
            }

            if resource.pause_from.is_some() || resource.pause_to.is_some() {
                // TODO: Get assignment type from resource.
                // For now we assume the most popular use case: wake up the namespace.
                let resource_ref = if NAMESPACE_TYPES.contains(&resource.ty.as_str()) {
                    ResourceReference {
                        api_version: Namespace::API_VERSION.to_string(),
                        kind: Namespace::KIND.to_string(),
                        name: name.to_string(),
                        namespace: None,
                    }
                } else {
                    continue;
                };

                let from = if let Some(pause_from) = resource.pause_from {
                    Some(convert_to_local_time(&pause_from, time_zone.as_ref())?)
                } else {
                    None
                };
                let to = if let Some(pause_to) = resource.pause_to {
                    Some(convert_to_local_time(&pause_to, time_zone.as_ref())?)
                } else {
                    None
                };

                let assignment = Assignment {
                    ty: AssignmentType::Work,
                    from,
                    to,
                    resource_filter: Some(ResourceFilter::MatchResources(vec![resource_ref])),
                };
                assignments.push(assignment);
            }
        }

        let spec = SchedulePolicySpec {
            title: policy.name.clone(),
            suspend: policy.disabled,
            time_zone,
            namespace_selector: NamespaceSelector::MatchNames(namespace_names),
            schedule,
            assignments: Some(assignments),
        };
        Ok(spec)
    }
}

mod safe_date_time {
    use serde::{self, Deserialize, Deserializer};

    use super::DATE_TIME_FORMAT;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<chrono::NaiveDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<String> = Option::deserialize(deserializer)?;
        if let Some(s) = s {
            let (d, _) = chrono::NaiveDateTime::parse_and_remainder(&s, DATE_TIME_FORMAT)
                .map_err(serde::de::Error::custom)?;
            Ok(Some(d))
        } else {
            Ok(None)
        }
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

    #[test]
    fn converts_to_local_time() {
        let now = chrono::DateTime::parse_from_rfc3339("2023-09-01T00:00:00Z").unwrap();
        assert_eq!(
            super::convert_to_local_time(&now, None).unwrap(),
            chrono::NaiveDateTime::parse_from_str("2023-09-01T00:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
        );
        assert_eq!(
            super::convert_to_local_time(&now, Some(&"Europe/Kyiv".to_string())).unwrap(),
            chrono::NaiveDateTime::parse_from_str("2023-09-01T03:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
        );
        assert_eq!(
            super::convert_to_local_time(&now, Some(&"Z".to_string())).unwrap(),
            chrono::NaiveDateTime::parse_from_str("2023-09-01T00:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
        );
        assert_eq!(
            super::convert_to_local_time(&now, Some(&"+03:00".to_string())).unwrap(),
            chrono::NaiveDateTime::parse_from_str("2023-09-01T03:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
        );
        assert_eq!(
            super::convert_to_local_time(&now, Some(&"-2".to_string())).unwrap(),
            chrono::NaiveDateTime::parse_from_str("2023-08-31T22:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
        );
    }

    #[test]
    fn parses_sleep_all_assignment() {
        let assignment = r#"{
            "type": "sleep"
        }"#;
        let assignment: super::Assignment = serde_json::from_str(assignment).unwrap();
        assert_eq!(assignment.ty, super::AssignmentType::Sleep);
        assert_eq!(assignment.from, None);
        assert_eq!(assignment.to, None);
        assert_eq!(assignment.resource_filter, None);
    }

    #[test]
    fn parses_assignment() {
        let assignment = r#"{
            "type": "work",
            "from": "2023-09-01T00:00:00+03:00",
            "to": "2023-09-01T00:00:00Z",
            "resourceFilter": {
                "matchResources": [{
                    "apiVersion": "v1",
                    "kind": "Namespace",
                    "name": "test",
                    "namespace": null
                }]
            }
        }"#;
        let assignment: super::Assignment = serde_json::from_str(assignment).unwrap();
        assert_eq!(assignment.ty, super::AssignmentType::Work);
        assert_eq!(
            assignment.from,
            Some(chrono::NaiveDateTime::parse_from_str("2023-09-01T00:00:00", "%Y-%m-%dT%H:%M:%S").unwrap())
        );
        assert_eq!(
            assignment.to,
            Some(chrono::NaiveDateTime::parse_from_str("2023-09-01T00:00:00", "%Y-%m-%dT%H:%M:%S").unwrap())
        );
        assert_eq!(
            assignment.resource_filter,
            Some(super::ResourceFilter::MatchResources(vec![
                super::ResourceReference {
                    api_version: "v1".to_string(),
                    kind: "Namespace".to_string(),
                    name: "test".to_string(),
                    namespace: None
                }
            ]))
        );
    }
}
