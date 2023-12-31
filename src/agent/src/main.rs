use actix_web::{get, middleware, web::Data, App, HttpRequest, HttpResponse, HttpServer, Responder};
use prometheus::{Encoder, TextEncoder};

use controller_core::{self, telemetry};
use kubesitter::controller::{self, State};

#[allow(clippy::unused_async)]
#[get("/metrics")]
async fn metrics(c: Data<State>, _req: HttpRequest) -> impl Responder {
    let metrics = c.metrics();
    let encoder = TextEncoder::new();
    let mut buffer = vec![];
    encoder.encode(&metrics, &mut buffer).unwrap();
    HttpResponse::Ok().body(buffer)
}

#[allow(clippy::unused_async)]
#[get("/health")]
async fn health(_: HttpRequest) -> impl Responder {
    HttpResponse::Ok().json("healthy")
}

#[get("/")]
async fn index(c: Data<State>, _req: HttpRequest) -> impl Responder {
    let d = c.diagnostics().await;
    HttpResponse::Ok().json(&d)
}

fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
    .build()
    .unwrap();

    rt.block_on(async move {
        telemetry::init().await.expect("Telemetry initialized");
    
        // Initiatilize Kubernetes controller state
        let state = State::default();
        let controller = controller::run(state.clone());
    
        // Start web server
        let server = HttpServer::new(move || {
            App::new()
                .app_data(Data::new(state.clone()))
                .wrap(middleware::Logger::default().exclude("/health"))
                .service(index)
                .service(health)
                .service(metrics)
        })
        .bind("0.0.0.0:8080")?
        .shutdown_timeout(5);
    
        // Both runtimes implements graceful shutdown, so poll until both are done
        tokio::join!(controller, server.run()).1?;
        Ok(())
    })
}
