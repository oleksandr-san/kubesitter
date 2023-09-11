use controller_core::Error;

use chrono::Datelike;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkTime {
    pub start: chrono::NaiveTime,
    pub stop: chrono::NaiveTime,
    pub days: Vec<chrono::Weekday>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Schedule {
    WorkTimes(Vec<WorkTime>),
}

pub fn convert_to_local_time<Tz: chrono::TimeZone>(
    time: &chrono::DateTime<Tz>,
    time_zone: &str,
) -> Result<chrono::NaiveDateTime, Error> {
    let tz: chrono_tz::Tz = time_zone.parse().map_err(Error::DeserializationError)?;
    Ok(time.with_timezone(&tz).naive_local())
}

pub fn determine_desired_state(schedule: &Schedule, now: &chrono::NaiveDateTime) -> Result<bool, Error> {
    match schedule {
        Schedule::WorkTimes(times) => {
            let weekday = now.weekday();
            let now = now.time();

            let desired_state = times
                .iter()
                .any(|time| time.days.contains(&weekday) && time.start <= now && time.stop >= now);
            Ok(desired_state)
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
    fn determines_desired_state() {
        let schedule = r#"{
            "workTimes": [{
                "start": "08:00:00",
                "stop": "17:00:00",
                "days": ["Mon", "Tue", "Wed", "Thu", "Fri"]
            }]
        }
        "#;
        let schedule: super::Schedule = serde_json::from_str(schedule).unwrap();

        for (now, expected_desired_state) in vec![
            ("2023-09-01T07:59:59", false),
            ("2023-09-01T08:00:00", true),
            ("2023-09-01T08:00:01", true),
            ("2023-09-01T16:59:59", true),
            ("2023-09-01T17:00:00", true),
            ("2023-09-01T17:00:01", false),
        ] {
            let now = chrono::NaiveDateTime::parse_from_str(now, "%Y-%m-%dT%H:%M:%S").unwrap();
            let desired_state = super::determine_desired_state(&schedule, &now).unwrap();
            assert_eq!(desired_state, expected_desired_state, "now: {}", now);
        }
    }

    #[test]
    fn determines_awlways_on_state() {
        let schedule = r#"{
            "workTimes": [{
                "start": "00:00:00",
                "stop": "23:59:59",
                "days": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            }]
        }
        "#;
        let schedule: super::Schedule = serde_json::from_str(schedule).unwrap();

        for (now, expected_desired_state) in vec![
            ("2023-09-01T00:00:00", true),
            ("2023-09-03T00:00:00", true),
            ("2023-09-04T00:00:00", true),
            ("2023-09-01T23:59:59", true),
            ("2023-09-01T01:00:00", true),
            ("2023-09-01T12:00:00", true),
        ] {
            let now = chrono::NaiveDateTime::parse_from_str(now, "%Y-%m-%dT%H:%M:%S").unwrap();
            let desired_state = super::determine_desired_state(&schedule, &now).unwrap();
            assert_eq!(desired_state, expected_desired_state, "now: {}", now);
        }
    }

    #[test]
    fn converts_to_local_time() {
        let now = chrono::DateTime::parse_from_rfc3339("2023-09-01T00:00:00Z").unwrap();
        let now = super::convert_to_local_time(&now, "Europe/Kyiv").unwrap();
        assert_eq!(
            now,
            chrono::NaiveDateTime::parse_from_str("2023-09-01T03:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
        );
    }
}
