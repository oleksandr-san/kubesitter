use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::*;
use uniskai_sdk::UniskaiClient;

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
            inner: Arc::new(RwLock::new(ConnectionStateInner { connected: false })),
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

pub struct UniskaiController {
    uniskai_client: UniskaiClient,
    check_interval: std::time::Duration,
    connection_state: ConnectionState,
}

impl UniskaiController {
    pub fn new(uniskai_client: UniskaiClient, check_interval: std::time::Duration) -> Self {
        Self {
            uniskai_client,
            check_interval,
            connection_state: ConnectionState::new(),
        }
    }

    pub fn connection_state(&self) -> &ConnectionState {
        &self.connection_state
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
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
                    if error_count > 5 {
                        self.connection_state.set_connected(false).await;
                        println!("Error listing policies: {}", e);
                        return Err(Box::new(e));
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
                    } else {
                        let existing_policy_json = serde_json::to_string(existing_policy)?;
                        let policy_json = serde_json::to_string(&policy)?;
                        let mismatch = json_diff::compare_jsons(&existing_policy_json, &policy_json);
                        println!("Policy {} changed: {:?}", policy.name, mismatch);
                        store.insert(policy.id, policy);
                    }
                } else {
                    println!("New policy: {}", policy.name);
                    store.insert(policy.id, policy);
                }
            }

            tokio::time::sleep(self.check_interval).await;
        }
    }
}
