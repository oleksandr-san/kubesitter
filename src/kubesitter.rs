#![allow(unused_imports, unused_variables)]
pub use controller::{self, controller::kubesitter, telemetry};

#[tokio::main]
async fn main() {
    telemetry::init().await;

    let mut args = std::env::args();
    args.next();

    let desired_state = match args.next() {
        Some(s) => {
            if s == "on" || s == "1" {
                true
            } else if s == "off" || s == "0" {
                false
            } else {
                panic!("Invalid desired state provided")
            }
        }
        None => panic!("No desired state provided"),
    };
    println!("Desired state: {}", desired_state);

    let namespace_names = args.collect::<Vec<String>>();
    println!("Namespaces: {:?}", namespace_names);

    let client = kube::Client::try_default().await.unwrap();
    kubesitter::reconcile_namespaced_resources(client, &namespace_names[0], desired_state)
        .await
        .unwrap();
}
