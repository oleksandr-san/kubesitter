use super::{
    REPLICAS_ANNOTATION,
    NODE_SELECTOR_ANNOTATION,
};

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use kube::{
    api::{Patch, ResourceExt},
    Resource,
};

use once_cell::sync::Lazy;
use serde_json::json;
use serde_json::Value;
use std::collections::BTreeMap;
use tracing::{debug, info, warn};

static DEAMONSET_SLEEPING_NODE_SELECTOR: Lazy<BTreeMap<String, String>> = Lazy::new(|| {
    let mut m = BTreeMap::new();
    m.insert("non-existent-label".to_string(), "non-existent-name".to_string());
    m
});

pub(super) fn generate_deployment_patch(deploy: &Deployment, desired_state: bool) -> Option<Patch<Value>> {
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

pub(super) fn generate_stateful_set_patch(resource: &StatefulSet, desired_state: bool) -> Option<Patch<Value>> {
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

pub(super) fn generate_replica_set_patch(resource: &ReplicaSet, desired_state: bool) -> Option<Patch<Value>> {
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

pub(super) fn generate_daemon_set_patch(resource: &DaemonSet, desired_state: bool) -> Option<Patch<Value>> {
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