#![allow(unused_imports, unused_variables)]
use controller_core::telemetry;
use kubesitter::{uniskai, resources_logic};
use uniskai_sdk::UniskaiClient;

#[tokio::main]
async fn main() {
    telemetry::init().await;

    let mut args = std::env::args();
    args.next();

    let command = args.next().expect("No command provided");
    if command == "cs-query" {
        let api_key = args.next().expect("No API key provided");
        let env_id = args.next().expect("No environment ID provided");
        let api_url = args.next().expect("No API URL provided");

        let client = UniskaiClient::new(api_key, api_url, env_id);
        let policies = client.list_cloudsitter_policies().await.unwrap();
        for policy in policies {
            println!("Policy: {:?}", policy);
        }
    } else if command == "cs-reconcile" {
        let api_key = args.next().expect("No API key provided");
        let env_id = args.next().expect("No environment ID provided");
        let api_url = args.next().expect("No API URL provided");

        let kube_client = kube::Client::try_default().await.unwrap();
        let client = UniskaiClient::new(api_key, api_url, env_id);
        let controller = uniskai::UniskaiController::new(
            kube_client,
            client,
            std::time::Duration::from_secs(5),
        );
        controller.run().await.unwrap();
    } else if command == "kubesit" {
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
        resources_logic::reconcile_namespace(client, namespace_names[0].clone(), desired_state)
            .await
            .unwrap();
    } else {
        panic!("Unknown command: {}", command);
    }
}