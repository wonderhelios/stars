mod error;
mod okx;
mod state;
mod types;
mod web;

use state::AppState;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let state = AppState::new();

    // 启动 WS 采集任务
    let ws_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = okx::ws::run_forever(ws_state).await {
            tracing::error!("ws task exited: {}", e);
        }
    });

    // 启动 HTTP 服务
    let app = web::api::router(state.clone());
    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Web server listening on http://{}", addr);
    info!("Open http://<server-ip>:3000 in your local browser");

    axum::serve(listener, app).await?;
    Ok(())
}
