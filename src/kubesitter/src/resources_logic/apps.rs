use super::{NODE_SELECTOR_ANNOTATION, REPLICAS_ANNOTATION};

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::core::v1::ReplicationController;
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

pub(super) fn generate_deployment_patch(resource: &Deployment, desired_state: bool) -> Option<Patch<Value>> {
    let api_version = Deployment::api_version(&());
    let kind = Deployment::kind(&());
    
    let current_replicas = resource.spec.as_ref()?.replicas?;
    if desired_state {
        let Some(original_replicas) = resource.annotations().get(REPLICAS_ANNOTATION) else {
            warn!(
                "Skipping deployment {} in namespace {} because it does not have the {} annotation",
                resource.name_any(),
                resource.namespace().unwrap(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        let Ok(original_replicas) = original_replicas.parse::<i32>() else {
            warn!(
                "Skipping deployment {} in namespace {} because the {} annotation is not an integer",
                resource.name_any(),
                resource.namespace().unwrap(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        if original_replicas == current_replicas {
            info!(
                "Skipping {} {}/{} because it is already scaled to {}",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                original_replicas,
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "spec": {
                "replicas": original_replicas,
            }
        }));
        Some(patch)
    } else {
        if current_replicas == 0 {
            info!(
                "Skipping deployment {} in namespace {} because it is already scaled to 0",
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

pub(super) fn generate_stateful_set_patch(
    resource: &StatefulSet,
    desired_state: bool,
) -> Option<Patch<Value>> {
    let api_version = StatefulSet::api_version(&());
    let kind = StatefulSet::kind(&());
    
    let current_replicas = resource.spec.as_ref()?.replicas?;
    if desired_state {
        let Some(original_replicas) = resource.annotations().get(REPLICAS_ANNOTATION) else {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        let Ok(original_replicas) = original_replicas.parse::<i32>() else {
            warn!(
                "Skipping {} {}/{} because the {} annotation is not an integer",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        if original_replicas == current_replicas {
            info!(
                "Skipping {} {}/{} because it is already scaled to {}",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                original_replicas,
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "spec": {
                "replicas": original_replicas,
            }
        }));
        Some(patch)
    } else {
        if current_replicas == 0 {
            info!(
                "Skipping {} {}/{} because it is already scaled to 0",
                kind,
                resource.name_any(),
                resource.namespace().unwrap_or_default(),
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
            resource.namespace().unwrap_or_default(),
            resource.name_any(),
        );
        return None;
    }
    
    let current_replicas = resource.spec.as_ref()?.replicas?;
    if desired_state {
        let Some(original_replicas) = resource.annotations().get(REPLICAS_ANNOTATION) else {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        let Ok(original_replicas) = original_replicas.parse::<i32>() else {
            warn!(
                "Skipping {} {}/{} because the {} annotation is not an integer",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        if original_replicas == current_replicas {
            info!(
                "Skipping {} {}/{} because it is already scaled to {}",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                original_replicas,
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "spec": {
                "replicas": original_replicas,
            }
        }));
        Some(patch)
    } else {
        if current_replicas == 0 {
            info!(
                "Skipping {} {}/{} because it is already scaled to 0",
                kind,
                resource.name_any(),
                resource.namespace().unwrap_or_default(),
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

pub(super) fn generate_replication_controller_patch(resource: &ReplicationController, desired_state: bool) -> Option<Patch<Value>> {
    let api_version = ReplicationController::api_version(&());
    let kind = ReplicationController::kind(&());
    
    let current_replicas = resource.spec.as_ref()?.replicas?;
    if desired_state {
        let Some(original_replicas) = resource.annotations().get(REPLICAS_ANNOTATION) else {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        let Ok(original_replicas) = original_replicas.parse::<i32>() else {
            warn!(
                "Skipping {} {}/{} because the {} annotation is not an integer",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };
        if original_replicas == current_replicas {
            info!(
                "Skipping {} {}/{} because it is already scaled to {}",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                original_replicas,
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
            "spec": {
                "replicas": original_replicas,
            }
        }));
        Some(patch)
    } else {
        if current_replicas == 0 {
            info!(
                "Skipping {} {}/{} because it is already scaled to 0",
                kind,
                resource.name_any(),
                resource.namespace().unwrap_or_default(),
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

#[allow(clippy::too_many_lines)]
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
        .as_ref();

    if desired_state {
        if node_selector.is_none() {
            info!(
                "Skipping {} {}/{} because it does not have a node selector",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
            );
            return None;
        }

        let Some(original_node_selector) = resource.annotations().get(NODE_SELECTOR_ANNOTATION) else {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                REPLICAS_ANNOTATION,
            );
            return None;
        };

        let original_node_selector =
            match serde_json::from_str::<Option<BTreeMap<String, String>>>(original_node_selector) {
                Ok(Some(mut node_selector)) => {
                    for key in DEAMONSET_SLEEPING_NODE_SELECTOR.keys() {
                        node_selector.remove(key);
                    }
                    Some(node_selector)
                }
                Ok(None) => None,
                Err(err) => {
                    warn!(
                        "Skipping {} {}/{} because the {} annotation is not a valid JSON: {}",
                        kind,
                        resource.namespace().unwrap_or_default(),
                        resource.name_any(),
                        NODE_SELECTOR_ANNOTATION,
                        err,
                    );
                    return None;
                }
            };

        if node_selector == original_node_selector.as_ref() {
            info!(
                "Skipping {} {}/{} because node selector has not changed",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
            );
            return None;
        }

        let patch: Patch<Value> = Patch::Apply(json!({
            "apiVersion": api_version,
            "kind": kind,
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
        if node_selector.is_some_and(|node_selector| {
            node_selector
                .keys()
                .all(|k| DEAMONSET_SLEEPING_NODE_SELECTOR.contains_key(k))
        }) {
            info!(
                "Skipping {} {}/{} because it is already suspended",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
            );
            return None;
        }

        let Ok(node_selector) = serde_json::to_string(&node_selector) else {
            warn!(
                "Skipping {} {}/{} due to annotation serialization error: {:?}",
                kind,
                resource.namespace().unwrap_or_default(),
                resource.name_any(),
                node_selector,
            );
            return None;
        };

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

#[cfg(test)]
mod tests {
    use k8s_openapi::api::{
        apps::v1::DaemonSet,
        core::v1::{PodSpec, PodTemplateSpec},
    };

    #[test]
    fn patches_deamonset_with_null_selector() {
        let ds = DaemonSet {
            spec: Some(k8s_openapi::api::apps::v1::DaemonSetSpec {
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        node_selector: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let patch = super::generate_daemon_set_patch(&ds, true);
        assert!(patch.is_none());

        let patch = super::generate_daemon_set_patch(&ds, false);
        assert!(patch.is_some());
    }
}
