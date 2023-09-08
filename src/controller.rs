use crate::{schedule::Schedule, telemetry, Error, Metrics, Result};

use chrono::{DateTime, Utc};
use futures::StreamExt;
use kube::{
    api::{Api, ListParams, Patch, PatchParams, ResourceExt},
    client::Client,
    runtime::{
        controller::{Action, Controller},
        events::{Event, EventType, Recorder, Reporter},
        finalizer::{finalizer, Event as Finalizer},
        watcher::Config,
    },
    CustomResource, Resource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::{sync::RwLock, time::Duration};
use tracing::*;

pub static DOCUMENT_FINALIZER: &str = "schedulepolicies.api.profisealabs.com";

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub enum LabelSelectorRequirementOperator {
    In,
    NotIn,
    Exists,
    DoesNotExist,
}

/// LabelSelectorRequirement is a selector that contains values, a key, and an operator that
/// relates the key and values.
/// Valid operators are In, NotIn, Exists and DoesNotExist.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct LabelSelectorRequirement {
    pub key: String,
    pub operator: String,
    pub values: Option<Vec<String>>,
}

impl LabelSelectorRequirement {
    pub fn to_label_selector(&self) -> String {
        let mut selector = String::new();
        selector.push_str(&self.key);
        selector.push(' ');
        selector.push_str(&self.operator.to_ascii_lowercase());
        if let Some(values) = &self.values {
            selector.push_str(" (");
            selector.push_str(&values.join(","));
            selector.push(')');
        }
        selector
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NamespaceSelector {
    MatchNames(Vec<String>),
    MatchLabels(BTreeMap<String, String>),
    MatchExpressions(Vec<LabelSelectorRequirement>),
}

/// Generate the Kubernetes wrapper struct `SchedulePolicy` from our Spec and Status struct
///
/// This provides a hook for generating the CRD yaml (in crdgen.rs)
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
// #[cfg_attr(test, derive(Default))]
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
    fn was_suspended(&self) -> bool {
        self.status.as_ref().map(|s| s.suspended).unwrap_or(false)
    }
}

// Context for our reconciler
#[derive(Clone)]
pub struct Context {
    /// Kubernetes client
    pub client: Client,
    /// Diagnostics read by the web server
    pub diagnostics: Arc<RwLock<Diagnostics>>,
    /// Prometheus metrics
    pub metrics: Metrics,
}

#[instrument(skip(ctx, doc), fields(trace_id))]
async fn reconcile(doc: Arc<SchedulePolicy>, ctx: Arc<Context>) -> Result<Action> {
    let trace_id = telemetry::get_trace_id();
    Span::current().record("trace_id", &field::display(&trace_id));
    let _timer = ctx.metrics.count_and_measure();
    ctx.diagnostics.write().await.last_event = Utc::now();
    let ns = doc.namespace().unwrap(); // doc is namespace scoped
    let docs: Api<SchedulePolicy> = Api::namespaced(ctx.client.clone(), &ns);

    info!("Reconciling SchedulePolicy \"{}\" in {}", doc.name_any(), ns);
    finalizer(&docs, DOCUMENT_FINALIZER, doc, |event| async {
        match event {
            Finalizer::Apply(doc) => doc.reconcile(ctx.clone()).await,
            Finalizer::Cleanup(doc) => doc.cleanup(ctx.clone()).await,
        }
    })
    .await
    .map_err(|e| Error::FinalizerError(Box::new(e)))
}

fn error_policy(doc: Arc<SchedulePolicy>, error: &Error, ctx: Arc<Context>) -> Action {
    warn!("reconcile failed: {:?}", error);
    ctx.metrics.reconcile_failure(&*doc, error);
    Action::requeue(Duration::from_secs(30))
}

impl SchedulePolicy {
    // Reconcile (for non-finalizer related changes)
    async fn reconcile(&self, ctx: Arc<Context>) -> Result<Action> {
        let client = ctx.client.clone();
        let recorder = ctx.diagnostics.read().await.recorder(client.clone(), self);
        let ns = self.namespace().unwrap();
        let name = self.name_any();
        let docs: Api<SchedulePolicy> = Api::namespaced(client.clone(), &ns);

        match kubesitter::reconcile_policy_resources(client.clone(), self).await {
            Err(err) => warn!("Failed to reconcile policy resources: {}", err),
            Ok(()) => (),
        }

        let should_suspend = self.spec.suspend;
        if !self.was_suspended() && should_suspend {
            // send an event once per hide
            recorder
                .publish(Event {
                    type_: EventType::Normal,
                    reason: "SuspendRequested".into(),
                    note: Some(format!("Suspending `{name}`")),
                    action: "Suspending".into(),
                    secondary: None,
                })
                .await
                .map_err(Error::KubeError)?;
        }

        if name == "illegal" {
            return Err(Error::IllegalResource);
        }

        // always overwrite status object with what we saw
        let new_status = Patch::Apply(json!({
            "apiVersion": SchedulePolicy::api_version(&()),
            "kind": "SchedulePolicy",
            "status": SchedulePolicyStatus {
                suspended: should_suspend,
            }
        }));
        let ps = PatchParams::apply("cntrlr").force();
        let _o = docs
            .patch_status(&name, &ps, &new_status)
            .await
            .map_err(Error::KubeError)?;

        // If no events were received, check back every 5 minutes
        Ok(Action::requeue(Duration::from_secs(30)))
    }

    // Finalizer cleanup (the object was deleted, ensure nothing is orphaned)
    async fn cleanup(&self, ctx: Arc<Context>) -> Result<Action> {
        let recorder = ctx.diagnostics.read().await.recorder(ctx.client.clone(), self);
        // SchedulePolicy doesn't have any real cleanup, so we just publish an event
        recorder
            .publish(Event {
                type_: EventType::Normal,
                reason: "DeleteRequested".into(),
                note: Some(format!("Delete `{}`", self.name_any())),
                action: "Deleting".into(),
                secondary: None,
            })
            .await
            .map_err(Error::KubeError)?;
        Ok(Action::await_change())
    }
}

/// Diagnostics to be exposed by the web server
#[derive(Clone, Serialize)]
pub struct Diagnostics {
    #[serde(deserialize_with = "from_ts")]
    pub last_event: DateTime<Utc>,
    #[serde(skip)]
    pub reporter: Reporter,
}
impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            last_event: Utc::now(),
            reporter: "schedulepolicy-controller".into(),
        }
    }
}
impl Diagnostics {
    fn recorder(&self, client: Client, doc: &SchedulePolicy) -> Recorder {
        Recorder::new(client, self.reporter.clone(), doc.object_ref(&()))
    }
}

/// State shared between the controller and the web server
#[derive(Clone, Default)]
pub struct State {
    /// Diagnostics populated by the reconciler
    diagnostics: Arc<RwLock<Diagnostics>>,
    /// Metrics registry
    registry: prometheus::Registry,
}

/// State wrapper around the controller outputs for the web server
impl State {
    /// Metrics getter
    pub fn metrics(&self) -> Vec<prometheus::proto::MetricFamily> {
        self.registry.gather()
    }

    /// State getter
    pub async fn diagnostics(&self) -> Diagnostics {
        self.diagnostics.read().await.clone()
    }

    // Create a Controller Context that can update State
    pub fn to_context(&self, client: Client) -> Arc<Context> {
        Arc::new(Context {
            client,
            metrics: Metrics::default().register(&self.registry).unwrap(),
            diagnostics: self.diagnostics.clone(),
        })
    }
}

/// Initialize the controller and shared state (given the crd is installed)
pub async fn run(state: State) {
    let client = Client::try_default().await.expect("failed to create kube Client");
    let docs = Api::<SchedulePolicy>::all(client.clone());
    if let Err(e) = docs.list(&ListParams::default().limit(1)).await {
        error!("CRD is not queryable; {e:?}. Is the CRD installed?");
        info!("Installation: cargo run --bin crdgen | kubectl apply -f -");
        std::process::exit(1);
    }
    Controller::new(docs, Config::default().any_semantic())
        .shutdown_on_signal()
        .run(reconcile, error_policy, state.to_context(client))
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;
}

pub mod kubesitter {
    use itertools::Itertools;
    use k8s_openapi::{
        api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
        api::batch::v1::CronJob,
        api::core::v1::Namespace,
    };
    use kube::core::object::HasSpec;
    use regex::Regex;

    use kube::api::{Api, ListParams, ResourceExt};
    use kube::core::ObjectList;
    use once_cell::sync::Lazy;
    use serde::de::DeserializeOwned;
    use serde_json::Value;
    use std::fmt::Debug;

    use crate::schedule;

    use super::*;

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

        let current_time = Utc::now();
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
}

// Mock tests relying on fixtures.rs and its primitive apiserver mocks
#[cfg(test)]
mod test {
    use super::{error_policy, reconcile, Context, SchedulePolicy};
    use crate::fixtures::{timeout_after_1s, Scenario};
    use std::sync::Arc;

    #[tokio::test]
    async fn documents_without_finalizer_gets_a_finalizer() {
        let (testctx, fakeserver, _) = Context::test();
        let doc = SchedulePolicy::test();
        let mocksrv = fakeserver.run(Scenario::FinalizerCreation(doc.clone()));
        reconcile(Arc::new(doc), testctx).await.expect("reconciler");
        timeout_after_1s(mocksrv).await;
    }

    // #[tokio::test]
    // async fn finalized_doc_causes_status_patch() {
    //     let (testctx, fakeserver, _) = Context::test();
    //     let doc = SchedulePolicy::test().finalized();
    //     let mocksrv = fakeserver.run(Scenario::StatusPatch(doc.clone()));
    //     reconcile(Arc::new(doc), testctx).await.expect("reconciler");
    //     timeout_after_1s(mocksrv).await;
    // }

    // #[tokio::test]
    // async fn finalized_doc_with_hide_causes_event_and_hide_patch() {
    //     let (testctx, fakeserver, _) = Context::test();
    //     let doc = SchedulePolicy::test().finalized().needs_hide();
    //     let scenario = Scenario::EventPublishThenStatusPatch("SuspendRequested".into(), doc.clone());
    //     let mocksrv = fakeserver.run(scenario);
    //     reconcile(Arc::new(doc), testctx).await.expect("reconciler");
    //     timeout_after_1s(mocksrv).await;
    // }

    #[tokio::test]
    async fn finalized_doc_with_delete_timestamp_causes_delete() {
        let (testctx, fakeserver, _) = Context::test();
        let doc = SchedulePolicy::test().finalized().needs_delete();
        let mocksrv = fakeserver.run(Scenario::Cleanup("DeleteRequested".into(), doc.clone()));
        reconcile(Arc::new(doc), testctx).await.expect("reconciler");
        timeout_after_1s(mocksrv).await;
    }

    #[tokio::test]
    async fn illegal_doc_reconcile_errors_which_bumps_failure_metric() {
        let (testctx, fakeserver, _registry) = Context::test();
        let doc = Arc::new(SchedulePolicy::illegal().finalized());
        let mocksrv = fakeserver.run(Scenario::RadioSilence);
        let res = reconcile(doc.clone(), testctx.clone()).await;
        timeout_after_1s(mocksrv).await;
        assert!(res.is_err(), "apply reconciler fails on illegal doc");
        let err = res.unwrap_err();
        assert!(err.to_string().contains("IllegalResource"));
        // calling error policy with the reconciler error should cause the correct metric to be set
        error_policy(doc.clone(), &err, testctx.clone());
        //dbg!("actual metrics: {}", registry.gather());
        let failures = testctx
            .metrics
            .failures
            .with_label_values(&["illegal", "finalizererror(applyfailed(illegalresource))"])
            .get();
        assert_eq!(failures, 1);
    }

    // Integration test without mocks
    use kube::api::{Api, ListParams, Patch, PatchParams};
    #[tokio::test]
    #[ignore = "uses k8s current-context"]
    async fn integration_reconcile_should_set_status_and_send_event() {
        let client = kube::Client::try_default().await.unwrap();
        let ctx = super::State::default().to_context(client.clone());

        // create a test doc
        let doc = SchedulePolicy::test().finalized().needs_hide();
        let docs: Api<SchedulePolicy> = Api::namespaced(client.clone(), "default");
        let ssapply = PatchParams::apply("ctrltest");
        let patch = Patch::Apply(doc.clone());
        docs.patch("test", &ssapply, &patch).await.unwrap();

        // reconcile it (as if it was just applied to the cluster like this)
        reconcile(Arc::new(doc), ctx).await.unwrap();

        // verify side-effects happened
        let output = docs.get_status("test").await.unwrap();
        assert!(output.status.is_some());
        // verify hide event was found
        let events: Api<k8s_openapi::api::core::v1::Event> = Api::all(client.clone());
        let opts =
            ListParams::default().fields("involvedObject.kind=SchedulePolicy,involvedObject.name=test");
        let event = events
            .list(&opts)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.reason.as_deref() == Some("SuspendRequested"))
            .last()
            .unwrap();
        dbg!("got ev: {:?}", &event);
        assert_eq!(event.action.as_deref(), Some("Hiding"));
    }
}
