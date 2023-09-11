use controller_core::{Error, Result};
use crate::schedule::{self, Schedule, WorkTime};
use crate::controller::{NamespaceSelector, SchedulePolicy};

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
use tracing::*;

const REPLICAS_ANNOTATION: &str = "cloudsitter.uniskai.com/original-replicas";
const SUSPENDED_ANNOTATION: &str = "cloudsitter.uniskai.com/is-suspended";
const NODE_SELECTOR_ANNOTATION: &str = "cloudsitter.uniskai.com/original-node-selector";
const SKIP_ANNOTATION: &str = "cloudsitter.uniskai.com/skip";

static DEAMONSET_SLEEPING_NODE_SELECTOR: Lazy<BTreeMap<String, String>> = Lazy::new(|| {
    let mut m = BTreeMap::new();
    m.insert("non-existent-label".to_string(), "non-existent-name".to_string());
    m
});

pub async fn reconcile_policy_resources(client: Client, policy: &SchedulePolicy) -> Result<()> {
    let namespaces = select_namespaces(client.clone(), &policy.spec.namespace_selector).await?;
    let names = namespaces
        .items
        .iter()
        .map(|ns| ns.name_any())
        .collect::<Vec<_>>();
    info!(
        "Found namespaces {} using selector {:?}",
        names.join(","),
        policy.spec.namespace_selector,
    );

    let current_time = chrono::Utc::now();
    let current_time = if let Some(tz) = &policy.spec().time_zone {
        schedule::convert_to_local_time(&current_time, tz)?
    } else {
        current_time.naive_utc()
    };

    let desired_state = schedule::determine_desired_state(&policy.spec().schedule, &current_time)?;
    let tasks = namespaces
        .items
        .into_iter()
        .map(|ns| reconcile_namespace(client.clone(), ns.name_any(), desired_state))
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
            label_selector = Some(exprs.iter().map(|expr| expr.to_label_selector()).join(","));
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
