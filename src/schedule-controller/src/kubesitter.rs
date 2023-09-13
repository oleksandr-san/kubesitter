use crate::model::{
    Assignment, AssignmentType, LabelSelectorRequirement, NamespaceSelector, ResourceReference, Schedule,
    SchedulePolicy,
};
use controller_core::{Error, Result};

use chrono::Datelike;
use itertools::Itertools;
use k8s_openapi::{
    api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    api::batch::v1::CronJob,
    api::core::v1::Namespace,
};
use kube::core::object::HasSpec;
use kube::{
    api::{Api, ListParams, Patch, PatchParams, ResourceExt},
    client::Client,
    core::ObjectList,
    Resource,
};
use regex::Regex;

use once_cell::sync::Lazy;
use serde::de::DeserializeOwned;
use serde_json::json;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Debug;
use tracing::{debug, info, warn};

const REPLICAS_ANNOTATION: &str = "cloudsitter.uniskai.com/original-replicas";
const SUSPENDED_ANNOTATION: &str = "cloudsitter.uniskai.com/is-suspended";
const NODE_SELECTOR_ANNOTATION: &str = "cloudsitter.uniskai.com/original-node-selector";
const SKIP_ANNOTATION: &str = "cloudsitter.uniskai.com/skip";

static DEAMONSET_SLEEPING_NODE_SELECTOR: Lazy<BTreeMap<String, String>> = Lazy::new(|| {
    let mut m = BTreeMap::new();
    m.insert("non-existent-label".to_string(), "non-existent-name".to_string());
    m
});

pub fn convert_to_local_time<Tz: chrono::TimeZone>(
    time: &chrono::DateTime<Tz>,
    time_zone: Option<&String>,
) -> Result<chrono::NaiveDateTime, Error> {
    if let Some(time_zone) = time_zone {
        let time_zone: chrono_tz::Tz = time_zone
            .parse()
            .map_err(|err: chrono_tz::ParseError| Error::InvalidParameters(err.into()))?;
        Ok(time.with_timezone(&time_zone).naive_local())
    } else {
        Ok(time.naive_utc())
    }
}

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

pub fn determine_assignment_type(
    resource: &ResourceReference,
    assignments: Option<&Vec<Assignment>>,
    current_time: &chrono::DateTime<chrono::Utc>,
    schedule_state: bool,
) -> AssignmentType {
    let mut assignments = assignments
        .map(|assignments| {
            assignments
                .iter()
                .filter(|assignment| assignment.is_active_at(current_time))
                .filter(|assignment| {
                    assignment
                        .resource_references
                        .iter()
                        .any(|ref_| ref_.matches(resource))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if assignments.len() > 1 {
        warn!(
            "Found {} active assignments for resource {:?}",
            assignments.len(),
            resource,
        );
    }

    if let Some(assignment) = assignments.pop() {
        info!(
            "Found assigned state resource {:?}: {:?}",
            assignment.ty, resource,
        );
        assignment.ty
    } else if schedule_state {
        AssignmentType::Work
    } else {
        AssignmentType::Sleep
    }
}

pub async fn reconcile_policy_resources(client: Client, policy: &SchedulePolicy) -> Result<()> {
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
                &current_utc_time,
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

fn generate_deployment_patch(deploy: &Deployment, desired_state: bool) -> Option<Patch<Value>> {
    let api_version = Deployment::api_version(&());
    let kind = Deployment::kind(&());

    if desired_state {
        let Some(original_replicas) = deploy.annotations().get(REPLICAS_ANNOTATION) else {
            warn!(
                "Skipping deployment {} in namespace {} because it does not have the {} annotation",
                deploy.name_any(),
                deploy.namespace().unwrap(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        let Ok(original_replicas) = original_replicas.parse::<i32>() else {
            warn!(
                "Skipping deployment {} in namespace {} because the {} annotation is not an integer",
                deploy.name_any(),
                deploy.namespace().unwrap(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    // REPLICAS_ANNOTATION: null,
                },
            },
            "spec": {
                "replicas": original_replicas,
            }
        }));
        Some(patch)
    } else {
        let current_replicas = deploy.spec.as_ref()?.replicas?;
        if current_replicas == 0 {
            info!(
                "Skipping deployment {} in namespace {} because it is already scaled to 0",
                deploy.name_any(),
                deploy.namespace().unwrap(),
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    REPLICAS_ANNOTATION: current_replicas.to_string(),
                },
            },
            "spec": {
                "replicas": 0,
            }
        }));
        Some(patch)
    }
}

fn generate_stateful_set_patch(resource: &StatefulSet, desired_state: bool) -> Option<Patch<Value>> {
    let api_version = StatefulSet::api_version(&());
    let kind = StatefulSet::kind(&());

    if desired_state {
        let Some(original_replicas) = resource.annotations().get(REPLICAS_ANNOTATION) else {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        let Ok(original_replicas) = original_replicas.parse::<i32>() else {
            warn!(
                "Skipping {} {}/{} because the {} annotation is not an integer",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    // REPLICAS_ANNOTATION: null,
                },
            },
            "spec": {
                "replicas": original_replicas,
            }
        }));
        Some(patch)
    } else {
        let current_replicas = resource.spec.as_ref()?.replicas?;
        if current_replicas == 0 {
            info!(
                "Skipping {} {}/{} because it is already scaled to 0",
                kind,
                resource.name_any(),
                resource.namespace().unwrap(),
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    REPLICAS_ANNOTATION: current_replicas.to_string(),
                },
            },
            "spec": {
                "replicas": 0,
            }
        }));
        Some(patch)
    }
}

fn generate_replica_set_patch(resource: &ReplicaSet, desired_state: bool) -> Option<Patch<Value>> {
    let api_version = ReplicaSet::api_version(&());
    let kind = ReplicaSet::kind(&());
    if resource.meta().owner_references.is_some() {
        debug!(
            "Skipping {} {}/{} because it is owned by another resource",
            kind,
            resource.namespace().unwrap(),
            resource.name_any(),
        );
        return None;
    }

    if desired_state {
        let Some(original_replicas) = resource.annotations().get(REPLICAS_ANNOTATION) else {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        let Ok(original_replicas) = original_replicas.parse::<i32>() else {
            warn!(
                "Skipping {} {}/{} because the {} annotation is not an integer",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    // REPLICAS_ANNOTATION: null,
                },
            },
            "spec": {
                "replicas": original_replicas,
            }
        }));
        Some(patch)
    } else {
        let current_replicas = resource.spec.as_ref()?.replicas?;
        if current_replicas == 0 {
            info!(
                "Skipping {} {}/{} because it is already scaled to 0",
                kind,
                resource.name_any(),
                resource.namespace().unwrap(),
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    REPLICAS_ANNOTATION: current_replicas.to_string(),
                },
            },
            "spec": {
                "replicas": 0,
            }
        }));
        Some(patch)
    }
}

fn generate_cron_job_patch(resource: &CronJob, desired_state: bool) -> Option<Patch<Value>> {
    let api_version = CronJob::api_version(&());
    let kind = CronJob::kind(&());

    if desired_state {
        if resource.annotations().get(SUSPENDED_ANNOTATION).is_none() {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    SUSPENDED_ANNOTATION: "false",
                },
            },
            "spec": {
                "suspend": false,
            }
        }));
        Some(patch)
    } else {
        let is_suspended = resource.spec.as_ref()?.suspend?;
        if is_suspended {
            info!(
                "Skipping {} {}/{} because it is already suspended",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    SUSPENDED_ANNOTATION: "true",
                },
            },
            "spec": {
                "suspend": true,
            }
        }));
        Some(patch)
    }
}

fn generate_daemon_set_patch(resource: &DaemonSet, desired_state: bool) -> Option<Patch<Value>> {
    let api_version = DaemonSet::api_version(&());
    let kind = DaemonSet::kind(&());
    let node_selector = resource
        .spec
        .as_ref()?
        .template
        .spec
        .as_ref()?
        .node_selector
        .as_ref()?;

    if desired_state {
        let Some(original_node_selector) = resource.annotations().get(NODE_SELECTOR_ANNOTATION) else {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };

        let original_node_selector =
            match serde_json::from_str::<BTreeMap<String, String>>(original_node_selector) {
                Ok(mut node_selector) => {
                    for key in DEAMONSET_SLEEPING_NODE_SELECTOR.keys() {
                        node_selector.remove(key);
                    }
                    node_selector
                }
                Err(err) => {
                    warn!(
                        "Skipping {} {}/{} because the {} annotation is not a valid JSON: {}",
                        kind,
                        resource.namespace().unwrap(),
                        resource.name_any(),
                        NODE_SELECTOR_ANNOTATION,
                        err,
                    );
                    return None;
                }
            };

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    // NODE_SELECTOR_ANNOTATION: node_selector,
                },
            },
            "spec": {
                "template": {
                    "spec": {
                        "nodeSelector": original_node_selector,
                    }
                }
            }
        }));
        Some(patch)
    } else {
        if node_selector
            .keys()
            .all(|k| DEAMONSET_SLEEPING_NODE_SELECTOR.contains_key(k))
        {
            info!(
                "Skipping {} {}/{} because it is already suspended",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
            );
            return None;
        }

        let sleeping_node_selector = Lazy::force(&DEAMONSET_SLEEPING_NODE_SELECTOR);
        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "metadata": {
                "annotations": {
                    NODE_SELECTOR_ANNOTATION: node_selector,
                },
            },
            "spec": {
                "template": {
                    "spec": {
                        "nodeSelector": sleeping_node_selector,
                    }
                }
            }
        }));
        Some(patch)
    }
}

pub async fn reconcile_namespace(client: Client, ns: String, desired_state: bool) -> Result<()> {
    let deployment: Api<Deployment> = Api::namespaced(client.clone(), &ns);
    let stateful_set: Api<StatefulSet> = Api::namespaced(client.clone(), &ns);
    let replica_set: Api<ReplicaSet> = Api::namespaced(client.clone(), &ns);
    let daemon_set: Api<DaemonSet> = Api::namespaced(client.clone(), &ns);
    let cron_job: Api<CronJob> = Api::namespaced(client.clone(), &ns);

    let _ = tokio::join!(
        reconcile_namespaced_resources(deployment, desired_state, generate_deployment_patch),
        reconcile_namespaced_resources(stateful_set, desired_state, generate_stateful_set_patch),
        reconcile_namespaced_resources(replica_set, desired_state, generate_replica_set_patch),
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

    let ps = PatchParams::apply("cntrlr").force();
    for resource in resources
        .list(&ListParams::default())
        .await
        .map_err(Error::KubeError)?
    {
        if resource.annotations().get(SKIP_ANNOTATION).is_some() {
            info!(
                "Skipping {} {}/{} because it has the {} annotation",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                SKIP_ANNOTATION,
            );
            continue;
        }

        if let Some(patch) = patch_fn(&resource, desired_state) {
            info!(
                "Patching {} {}/{}: {:?}",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                patch,
            );
            match resources.patch(&resource.name_any(), &ps, &patch).await {
                Ok(_) => {
                    info!(
                        "Successfully patched {} {}/{}",
                        kind,
                        resource.namespace().unwrap(),
                        resource.name_any(),
                    );
                }
                Err(err) => {
                    warn!(
                        "Failed to patch {} {}/{}: {}",
                        kind,
                        resource.namespace().unwrap(),
                        resource.name_any(),
                        err,
                    );
                }
            }
        } else {
            info!(
                "No need to patch {} {}/{}",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
            );
        }
    }

    Ok(())
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

    #[test]
    fn converts_to_local_time() {
        let now = chrono::DateTime::parse_from_rfc3339("2023-09-01T00:00:00Z").unwrap();
        let now = super::convert_to_local_time(&now, Some(&"Europe/Kyiv".to_string())).unwrap();
        assert_eq!(
            now,
            chrono::NaiveDateTime::parse_from_str("2023-09-01T03:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
        );
    }
}
