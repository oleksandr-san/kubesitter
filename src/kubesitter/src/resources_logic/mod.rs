mod apps;
mod core;
mod batch;

pub use self::core::*;

const REPLICAS_ANNOTATION: &str = "cloudsitter.uniskai.com/original-replicas";
const SUSPENDED_ANNOTATION: &str = "cloudsitter.uniskai.com/is-suspended";
const NODE_SELECTOR_ANNOTATION: &str = "cloudsitter.uniskai.com/original-node-selector";
const SKIP_ANNOTATION: &str = "cloudsitter.uniskai.com/skip";
