use crate::model::{SchedulePolicy, SchedulePolicySpec};
use uniskai_sdk::{CloudsitterPolicy, UniskaiClient};

use kube::{
    api::{Api, Patch, PatchParams},
    runtime::reflector,
    Client as KubeClient,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

static CONNECTION_FAILURE_THRESHOLD: u32 = 2;

struct ConnectionStateInner {
    connected: bool,
}

#[derive(Clone)]
pub struct ConnectionState {
    inner: Arc<RwLock<ConnectionStateInner>>,
}

impl ConnectionState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ConnectionStateInner { connected: true })),
        }
    }

    pub async fn set_connected(&self, connected: bool) {
        let mut inner = self.inner.write().await;
        inner.connected = connected;
    }

    pub async fn is_connected(&self) -> bool {
        let inner = self.inner.read().await;
        inner.connected
    }
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(clippy::module_name_repetitions)]
pub struct UniskaiController {
    kube_client: KubeClient,
    uniskai_client: UniskaiClient,
    check_interval: std::time::Duration,
    connection_state: ConnectionState,
    schedules_namespace: String,
    schedules_store: Option<reflector::Store<SchedulePolicy>>,
}

impl UniskaiController {
    pub fn new(
        kube_client: KubeClient,
        uniskai_client: UniskaiClient,
        check_interval: std::time::Duration,
    ) -> Self {
        Self {
            kube_client,
            uniskai_client,
            check_interval,
            connection_state: ConnectionState::new(),
            schedules_namespace: "uniskai".to_string(),
            schedules_store: None,
        }
    }

    pub fn with_schedules_store(mut self, schedules_store: reflector::Store<SchedulePolicy>) -> Self {
        self.schedules_store = Some(schedules_store);
        self
    }

    pub fn connection_state(&self) -> &ConnectionState {
        &self.connection_state
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut store = HashMap::new();
        let mut error_count = 0;

        loop {
            let policies = match self.uniskai_client.list_cloudsitter_policies().await {
                Ok(policies) => {
                    error_count = 0;
                    self.connection_state.set_connected(true).await;
                    policies
                }
                Err(e) => {
                    error_count += 1;
                    if error_count > CONNECTION_FAILURE_THRESHOLD {
                        warn!(
                            "Connection failure threshold reached, setting connection state to disconnected"
                        );
                        self.connection_state.set_connected(false).await;
                    }
                    error!("Error listing policies: {}", e);
                    tokio::time::sleep(self.check_interval).await;
                    continue;
                }
            };
            for policy in policies {
                if let Some(existing_policy) = store.get(&policy.id) {
                    if existing_policy == &policy {
                        continue;
                    }

                    let existing_policy_json = serde_json::to_string(existing_policy)?;
                    let policy_json = serde_json::to_string(&policy)?;
                    let mismatch = json_diff::compare_jsons(&existing_policy_json, &policy_json);
                    info!("Policy {} changed: {:?}", policy.name, mismatch);

                    match self.reconcile_policy(&policy).await {
                        Ok(_) => {}
                        Err(err) => {
                            error!("Error reconciling policy: {}", err);
                            continue;
                        }
                    };
                    store.insert(policy.id, policy);
                } else {
                    info!("New policy: {}", policy.name);
                    match self.reconcile_policy(&policy).await {
                        Ok(_) => {}
                        Err(err) => {
                            error!("Error reconciling policy: {}", err);
                            continue;
                        }
                    };
                    store.insert(policy.id, policy);
                }
            }

            tokio::time::sleep(self.check_interval).await;
        }
    }

    async fn reconcile_policy(
        &self,
        policy: &CloudsitterPolicy,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Reconciling policy: {}", policy.name);

        let schedule_api: Api<SchedulePolicy> =
            Api::namespaced(self.kube_client.clone(), &self.schedules_namespace);
        let resource_name = policy.name.replace(' ', "-").to_lowercase();
        let schedule = SchedulePolicySpec::try_from(policy)?;
        let existing_schedule = if let Some(schedules_store) = &self.schedules_store {
            let key = reflector::ObjectRef::new(&resource_name).within(&self.schedules_namespace);
            schedules_store.get(&key)
        } else {
            schedule_api.get_opt(&resource_name).await?.map(Arc::new)
        };

        if let Some(existing_schedule) = existing_schedule {
            if existing_schedule.spec == schedule {
                info!("Schedule {} is up to date", resource_name);
                return Ok(());
            }
        }

        info!("Applying schedule {}", resource_name);
        let ssapply = PatchParams::apply("uniskai-controller").force();
        let schedule = SchedulePolicy::new(&resource_name, schedule);
        let _ = schedule_api
            .patch(&resource_name, &ssapply, &Patch::Apply(&schedule))
            .await?;

        Ok(())
    }
}
