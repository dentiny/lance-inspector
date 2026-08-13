mod api;
mod models;
#[cfg(all(feature = "profiling", unix))]
mod profiler;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "profiling", not(target_env = "msvc")))]
#[unsafe(export_name = "malloc_conf")]
pub static MALLOC_CONF: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19\0";

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    Router,
    routing::{get, post},
};
use clap::Parser;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use api::AppState;

#[derive(Debug, Parser)]
#[command(
    name = "lance-inspector",
    about = "Read-only web inspector for Lance datasets"
)]
struct Args {
    /// Address to listen on.
    #[arg(long, env = "LANCE_INSPECTOR_BIND", default_value = "0.0.0.0:8080")]
    bind: SocketAddr,

    /// Directory containing the built React application.
    #[arg(long, env = "LANCE_INSPECTOR_UI_DIR", default_value = "frontend/dist")]
    ui_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let state = Arc::new(AppState::new());

    let api = Router::new()
        .route("/health", get(api::health))
        .route("/dataset", get(api::dataset_info))
        .route("/dataset/references", post(api::discover_dataset))
        .route("/dataset/connect", post(api::connect_dataset))
        .route("/files", get(api::files))
        .route("/file", get(api::file_preview))
        .route("/transaction", get(api::transaction))
        .route("/rows", get(api::rows))
        .route("/sql/start", post(api::start_sql))
        .route("/sql/{cursor_id}/page", get(api::sql_page))
        .route("/sql/{cursor_id}/cancel", post(api::cancel_sql))
        .route("/media/{column}/{row_address}", get(api::media))
        .with_state(state);

    let index = args.ui_dir.join("index.html");
    let static_files = ServeDir::new(&args.ui_dir).not_found_service(ServeFile::new(index));
    let app = Router::new()
        .nest("/api", api)
        .fallback_service(static_files)
        .layer(TraceLayer::new_for_http());
    #[cfg(all(feature = "profiling", unix))]
    let app = app.merge(profiler::routes());

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    println!(
        "Lance Inspector: http://{} (waiting for dataset selection)",
        args.bind
    );
    axum::serve(listener, app).await?;
    Ok(())
}
