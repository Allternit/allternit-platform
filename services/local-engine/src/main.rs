use allternit_local_engine::assess::Assessor;
use allternit_local_engine::cache::ModelStore;
use allternit_local_engine::catalog::CatalogService;
use allternit_local_engine::hardware;
use allternit_local_engine::recommend::Recommender;
use allternit_local_engine::routes::{
    assess, catalog, chat, hardware as hardware_routes, health, models, recommend, runtimes, status,
};
use allternit_local_engine::runtime::ProcessManager;
use allternit_local_engine::sampler::SystemSampler;
use allternit_local_engine::AppState;
use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("allternit-local-engine");
    let models_dir = data_dir.join("models");

    tokio::fs::create_dir_all(&models_dir).await?;
    tokio::fs::create_dir_all(data_dir.join("logs").join("runtimes")).await?;

    let store = ModelStore::new(&models_dir);
    // Registered Studio outputs (POST /models/studio-output) from earlier runs.
    let studio_outputs = store.load_studio_outputs().await;
    if studio_outputs > 0 {
        info!(count = studio_outputs, "studio outputs restored");
    }
    let manager = ProcessManager::new(&data_dir);
    let hardware_profile = hardware::detect_and_persist(&data_dir);
    let catalog = CatalogService::new(&data_dir);
    let assessor = Assessor::new();
    let recommender = Recommender::new();
    let sampler = SystemSampler::start();

    // Eagerly refresh the catalog on startup, then keep it updated in the
    // background. Errors are logged but do not prevent the service from starting.
    if let Err(err) = catalog.refresh().await {
        warn!(error = %err, "initial catalog refresh failed");
    }
    catalog.clone().spawn_background_refresh();

    let state = Arc::new(AppState {
        store,
        manager,
        models_dir: models_dir.clone(),
        data_dir: data_dir.clone(),
        hardware_profile,
        catalog,
        assessor,
        recommender,
        sampler,
    });

    let app = Router::new()
        .merge(health::create_router())
        .merge(models::create_router(state.clone()))
        .merge(runtimes::create_router(state.clone()))
        .merge(chat::create_router(state.clone()))
        .merge(catalog::create_router(state.clone()))
        .merge(assess::create_router(state.clone()))
        .merge(recommend::create_router(state.clone()))
        .merge(hardware_routes::create_router(state.clone()))
        .merge(status::create_router(state));

    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("PORT").unwrap_or_else(|_| "3015".to_string());
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("Allternit Local Engine listening on http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
