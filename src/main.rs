mod error;
mod okx;
mod research;
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

    // 子命令分发
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "research" {
        return research::run(&args[2..]).await;
    }

    // 默认 serve 模式
    let state = AppState::new();

    let ws_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = okx::ws::run_forever(ws_state).await {
            tracing::error!("ws task exited: {}", e);
        }
    });

    let app = web::api::router(state.clone());
    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Web server listening on http://{}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}
