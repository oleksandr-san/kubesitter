use crate::{resources_logic, uniskai};
use crate::model::{SchedulePolicy, SchedulePolicyStatus, POLICY_FINALIZER};

use controller_core::{telemetry, Error, Metrics, Result};

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
    Resource,
};
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use tokio::{sync::RwLock, time::Duration};
use tracing::*;

// Context for our reconciler
#[derive(Clone)]
pub struct Context {
    /// Kubernetes client
    pub client: Client,
    /// Diagnostics read by the web server
    pub diagnostics: Arc<RwLock<Diagnostics>>,
    /// Prometheus metrics
    pub metrics: Metrics,
    /// Uniskai connection state
    pub uniskai_connection: uniskai::ConnectionState,    
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
    finalizer(&docs, POLICY_FINALIZER, doc, |event| async {
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
        let name = self.name_any();
        if !ctx.uniskai_connection.is_connected().await {
            warn!(
                "Uniskai is not connected, skipping reconcile for {}",
                self.name_any(),
            );
            return Ok(Action::requeue(Duration::from_secs(30)));
        }

        let client = ctx.client.clone();
        let recorder = ctx.diagnostics.read().await.recorder(client.clone(), self);
        let ns = self.namespace().unwrap();
        let docs: Api<SchedulePolicy> = Api::namespaced(client.clone(), &ns);

        if let Err(err) = resources_logic::reconcile_policy_resources(client.clone(), self).await {
            warn!("Failed to reconcile policy resources: {}", err);
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

        // If no events were received, check back every 30s
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
    pub fn to_context(&self, client: Client, uniskai_connection: uniskai::ConnectionState) -> Arc<Context> {
        Arc::new(Context {
            client,
            metrics: Metrics::default().register(&self.registry).unwrap(),
            diagnostics: self.diagnostics.clone(),
            uniskai_connection,
        })
    }
}

/// Initialize the controller and shared state (given the crd is installed)
pub async fn run(state: State) {
    let client = Client::try_default().await.expect("failed to create kube Client");
    let uniskai_controller = uniskai::UniskaiController::new(
        client.clone(),
        uniskai_sdk::UniskaiClient::try_default().expect("failed to create uniskai Client"),
        Duration::from_secs(30),
    );

    let uniskai_connection = uniskai_controller.connection_state().clone();
    let _ = tokio::spawn(async move {
        uniskai_controller.run().await
    });
    
    let docs = Api::<SchedulePolicy>::all(client.clone());
    if let Err(e) = docs.list(&ListParams::default().limit(1)).await {
        error!("CRD is not queryable; {e:?}. Is the CRD installed?");
        info!("Installation: cargo run --bin crdgen | kubectl apply -f -");
        std::process::exit(1);
    }

    let context = state.to_context(client, uniskai_connection);

    Controller::new(docs, Config::default().any_semantic())
    .shutdown_on_signal()
    .run(reconcile, error_policy, context)
    .filter_map(|x| async move { std::result::Result::ok(x) })
    .for_each(|_| futures::future::ready(()))
    .await;
}

// Mock tests relying on fixtures.rs and its primitive apiserver mocks
#[cfg(test)]
mod test {
    use super::{error_policy, reconcile, Context, SchedulePolicy, uniskai};
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
        let uniskai_connection = uniskai::ConnectionState::default();
        let ctx = super::State::default().to_context(client.clone(), uniskai_connection);

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
