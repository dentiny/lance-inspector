mod api;
mod cache;
mod models;
#[cfg(all(feature = "profiling", unix))]
mod profiler;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "profiling", not(target_env = "msvc")))]
#[unsafe(export_name = "malloc_conf")]
pub static MALLOC_CONF: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19\0";

use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    Router,
    routing::{get, post},
};
use tower_http::services::{ServeDir, ServeFile};

use api::AppState;

const BIND_ENV: &str = "LANCE_INSPECTOR_BIND";
const UI_DIR_ENV: &str = "LANCE_INSPECTOR_UI_DIR";

#[derive(Debug)]
struct Args {
    /// Address to listen on.
    bind: SocketAddr,

    /// Directory containing the built React application.
    ui_dir: PathBuf,
}

impl Args {
    fn from_env() -> Result<Self> {
        let bind = env::var(BIND_ENV).unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let bind = bind
            .parse()
            .with_context(|| format!("{BIND_ENV} must be a valid socket address, got {bind:?}"))?;
        let ui_dir = env::var_os(UI_DIR_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("frontend/dist"));
        Ok(Self { bind, ui_dir })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::from_env()?;
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
        .route("/media/{field_id}/{row_address}", get(api::media))
        .with_state(state);

    let index = args.ui_dir.join("index.html");
    let static_files = ServeDir::new(&args.ui_dir).not_found_service(ServeFile::new(index));
    let app = Router::new()
        .nest("/api", api)
        .fallback_service(static_files);
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
