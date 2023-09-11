#[tokio::main]
async fn main() {
    let mut args = std::env::args();
    args.next();

    let command = args.next().expect("No command provided");
    if command == "query" {
        let api_key = args.next().expect("No API key provided");
        let env_id = args.next().expect("No environment ID provided");

        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "https://feature-environments.profisealabs.com/api/environments/{}/cloudsitter/policies",
                env_id
            ))
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
            .unwrap();

        let body = response.text().await.unwrap();
        println!("{}", body);

        // let policies: Vec<CloudsitterPolicy> = serde_json::from_str(&body).unwrap();
        // for policy in policies {
        //     println!("Policy: {:?}", policy);
        // }
    } else if command == "parse" {
        let file = args.next().expect("No file path provided");
        let file = std::fs::File::open(file).expect("Failed to open file");
        println!("File opened");
        // let file = std::io::BufReader::new(file);
        // let text = file.lines().collect::<Result<Vec<String>, _>>().expect("Failed to read file");
        // let text = text.join("\n");
        // print!("File read, text: {}", text);

        let policies: Vec<CloudsitterPolicy> = serde_json::from_reader(file).expect("Failed to parse JSON");
        for policy in policies {
            let policy = convert_to_schedule_policy(&policy);
            println!("Policy: {:?}", policy);
        }
    } else {
        panic!("Unknown command: {}", command);
    }
}
