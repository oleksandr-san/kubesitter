use super::SUSPENDED_ANNOTATION;

use k8s_openapi::api::batch::v1::CronJob;
use kube::{
    api::{Patch, ResourceExt},
    Resource,
};

use serde_json::json;
use serde_json::Value;
use tracing::{info, warn};

pub(super) fn generate_cron_job_patch(resource: &CronJob, desired_state: bool) -> Option<Patch<Value>> {
    let api_version = CronJob::api_version(&());
    let kind = CronJob::kind(&());

    if desired_state {
        if resource.annotations().get(SUSPENDED_ANNOTATION).is_none() {
            warn!(
                "Skipping {} {}/{} because it does not have the {} annotation",
                kind,
                resource.namespace().unwrap(),
                resource.name_any(),
                SUSPENDED_ANNOTATION,
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