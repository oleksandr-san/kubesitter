use super::{
    apps::{
        generate_daemon_set_patch,
        generate_deployment_patch,
        generate_replica_set_patch,
        generate_replication_controller_patch,
        generate_stateful_set_patch,
    },
    batch::generate_cron_job_patch,
    SKIP_ANNOTATION,
};
use crate::model::{
    convert_to_local_time, Assignment, AssignmentType, LabelSelectorRequirement, NamespaceSelector,
    ResourceFilter, ResourceReference, Schedule, SchedulePolicy,
};
use controller_core::{Error, Result};

use chrono::Datelike;
use itertools::Itertools;
use k8s_openapi::{
    api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    api::{batch::v1::CronJob, core::v1::ReplicationController},
    api::core::v1::Namespace,
};
use kube::core::object::HasSpec;
use kube::{
    api::{Api, ListParams, Patch, PatchParams, ResourceExt},
    client::Client,
    core::ObjectList,
};
use regex::Regex;

use serde::de::DeserializeOwned;
use serde_json::Value;
use std::fmt::Debug;
use tracing::{info, warn};

pub fn determine_schedule_state(schedule: &Schedule, now: &chrono::NaiveDateTime) -> Result<bool, Error> {
    match schedule {
        Schedule::WorkTimes(times) => {
            let weekday = now.weekday();
            let now = now.time();

            let state = times
                .iter()
                .any(|time| time.days.contains(&weekday) && time.start <= now && time.stop >= now);
            Ok(state)
        }
    }
}

impl ResourceFilter {
    pub fn matches(&self, resource: &ResourceReference) -> bool {
        match self {
            ResourceFilter::MatchResources(filters) => filters.iter().any(|filter| filter.matches(resource)),
        }
    }
}

pub fn determine_assignment_type(
    resource: &ResourceReference,
    assignments: Option<&Vec<Assignment>>,
    current_time: &chrono::NaiveDateTime,
    schedule_state: bool,
) -> AssignmentType {
    let mut assignments = assignments
        .map(|assignments| {
            assignments
                .iter()
                .filter(|assignment| assignment.is_active_at(current_time))
                .filter(|assignment| {
                    if let Some(filter) = &assignment.resource_filter {
                        filter.matches(resource)
                    } else {
                        true
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if assignments.len() > 1 {
        warn!(
            "Found {} active assignments for resource {:?}, selecting last one",
            assignments.len(),
            resource,
        );
    }

    if let Some(assignment) = assignments.pop() {
        info!(
            "Determined assigned state {:?} for resource {:?}",
            assignment.ty, resource,
        );
        assignment.ty
    } else if schedule_state {
        AssignmentType::Work
    } else {
        AssignmentType::Sleep
    }
}

pub async fn select_namespaces(
    client: Client,
    selector: &NamespaceSelector,
) -> Result<ObjectList<Namespace>> {
    let mut name_patterns: Option<Vec<Regex>> = None;
    let mut label_selector: Option<String> = None;

    match selector {
        NamespaceSelector::MatchNames(names) => {
            let patterns = names
                .iter()
                .filter_map(|name| match Regex::new(name) {
                    Ok(re) => Some(re),
                    Err(err) => {
                        warn!("Skipping invalid regex for namespace name: {}", err);
                        None
                    }
                })
                .collect::<Vec<_>>();
            name_patterns = Some(patterns);
        }
        NamespaceSelector::MatchLabels(labels) => {
            label_selector = Some(
                labels
                    .iter()
                    .map(|(key, value)| format!("{}={}", key, value))
                    .join(","),
            );
        }
        NamespaceSelector::MatchExpressions(exprs) => {
            label_selector = Some(
                exprs
                    .iter()
                    .map(LabelSelectorRequirement::to_label_selector)
                    .join(","),
            );
        }
    };

    let namespaces: Api<Namespace> = Api::all(client);
    let mut list_params: ListParams = ListParams::default();
    if let Some(label_selector) = label_selector {
        info!("Using label selector: {}", label_selector);
        list_params = list_params.labels(&label_selector);
    }

    let mut namespaces: ObjectList<Namespace> =
        namespaces.list(&list_params).await.map_err(Error::KubeError)?;
    if let Some(name_patterns) = name_patterns {
        namespaces.items.retain(|ns| {
            let name: String = ns.name_any();
            for re in &name_patterns {
                if re.is_match(&name) {
                    return true;
                }
            }
            false
        });
    }

    Ok(namespaces)
}

pub async fn reconcile_policy_resources(client: Client, policy: &SchedulePolicy) -> Result<()> {
    if policy.spec.suspend {
        info!(
            "Skipping policy {}/{} because it is suspended",
            policy.namespace().unwrap_or_default(),
            policy.name_any(),
        );
        return Ok(());
    }

    let namespaces = select_namespaces(client.clone(), &policy.spec.namespace_selector).await?;
    info!(
        "Selected {} namespaces using selector {:?}",
        namespaces.items.len(),
        policy.spec.namespace_selector,
    );

    let policy_spec = policy.spec();
    let current_utc_time = chrono::Utc::now();
    let current_time = convert_to_local_time(&current_utc_time, policy_spec.time_zone.as_ref())?;
    let schedule_state = determine_schedule_state(&policy_spec.schedule, &current_time)?;

    let tasks = namespaces
        .items
        .into_iter()
        .filter_map(|ns| {
            let name = ns.name_any();
            let ref_ = ResourceReference::new_namespace(&name);
            let assignment_type = determine_assignment_type(
                &ref_,
                policy_spec.assignments.as_ref(),
                &current_time,
                schedule_state,
            );
            let desired_state = match assignment_type {
                AssignmentType::Work => true,
                AssignmentType::Sleep => false,
                AssignmentType::Skip => {
                    return None;
                }
            };
            Some(reconcile_namespace(client.clone(), name, desired_state))
        })
        .collect::<Vec<_>>();
    futures::future::join_all(tasks).await;

    Ok(())
}

pub async fn reconcile_namespace(client: Client, ns: String, desired_state: bool) -> Result<()> {
    let deployment: Api<Deployment> = Api::namespaced(client.clone(), &ns);
    let stateful_set: Api<StatefulSet> = Api::namespaced(client.clone(), &ns);
    let replica_set: Api<ReplicaSet> = Api::namespaced(client.clone(), &ns);
    let daemon_set: Api<DaemonSet> = Api::namespaced(client.clone(), &ns);
    let cron_job: Api<CronJob> = Api::namespaced(client.clone(), &ns);
    let replica_controller: Api<ReplicationController> = Api::namespaced(client.clone(), &ns);

    let _ = tokio::join!(
        reconcile_namespaced_resources(deployment, desired_state, generate_deployment_patch),
        reconcile_namespaced_resources(stateful_set, desired_state, generate_stateful_set_patch),
        reconcile_namespaced_resources(replica_set, desired_state, generate_replica_set_patch),
        reconcile_namespaced_resources(
            replica_controller,
            desired_state,
            generate_replication_controller_patch
        ),
        reconcile_namespaced_resources(cron_job, desired_state, generate_cron_job_patch),
        reconcile_namespaced_resources(daemon_set, desired_state, generate_daemon_set_patch),
    );
    Ok(())
}

pub async fn reconcile_namespaced_resources<R>(
    resources: Api<R>,
    desired_state: bool,
    patch_fn: impl Fn(&R, bool) -> Option<Patch<Value>>,
) -> Result<()>
where
    R: ResourceExt + Clone + DeserializeOwned + Debug,
    R::DynamicType: Default,
{
    let dynamic_type = R::DynamicType::default();
    let kind = R::kind(&dynamic_type);

    let ps = PatchParams::apply("kubesitter").force();
    for resource in resources
        .list(&ListParams::default())
        .await
        .map_err(Error::KubeError)?
    {
        if resource.annotations().get(SKIP_ANNOTATION).is_some() {
            info!(
                "Skipping {} {}/{} because it has the {} annotation",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                SKIP_ANNOTATION,
            );
            continue;
        }

        if let Some(patch) = patch_fn(&resource, desired_state) {
            info!(
                "Patching {} {}/{}: {:?}",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                patch,
            );
            match resources.patch(&resource.name_any(), &ps, &patch).await {
                Ok(_) => {
                    info!(
                        "Successfully patched {} {}/{}",
                        kind,
                        resource.namespace().unwrap_or_default(),
                        resource.name_any(),
                    );
                }
                Err(err) => {
                    warn!(
                        "Failed to patch {} {}/{}: {}",
                        kind,
                        resource.namespace().unwrap_or_default(),
                        resource.name_any(),
                        err,
                    );
                }
            }
        } else {
            info!(
                "No need to patch {} {}/{} for desired state {}",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                desired_state,
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::model::WorkTime;
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
                let WorkTime { start, stop, days } = &times[0];
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
            let desired_state = super::determine_schedule_state(&schedule, &now).unwrap();
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
            let desired_state = super::determine_schedule_state(&schedule, &now).unwrap();
            assert_eq!(desired_state, expected_desired_state, "now: {}", now);
        }
    }
}
