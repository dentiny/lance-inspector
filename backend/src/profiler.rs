use std::time::Duration;

use axum::{
    Router,
    body::Body,
    extract::Query,
    http::{
        HeaderValue, StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    },
    response::Response,
    routing::get,
};
use serde::Deserialize;
use tokio::sync::Mutex;

static CPU_PROFILER: Mutex<()> = Mutex::const_new(());

const DEFAULT_PROFILE_SECONDS: u64 = 10;
const MAX_PROFILE_SECONDS: u64 = 120;
const DEFAULT_SAMPLE_FREQUENCY: i32 = 99;
const MAX_SAMPLE_FREQUENCY: i32 = 1_000;

#[derive(Debug, Deserialize)]
struct CpuProfileQuery {
    seconds: Option<u64>,
    frequency: Option<i32>,
}

pub(crate) fn routes() -> Router {
    let router = Router::new().route("/debug/pprof/cpu/flamegraph", get(cpu_flamegraph));
    #[cfg(target_os = "linux")]
    let router = router.route("/debug/pprof/heap/flamegraph", get(heap_flamegraph));
    router
}

async fn cpu_flamegraph(Query(query): Query<CpuProfileQuery>) -> Response {
    let Ok(_permit) = CPU_PROFILER.try_lock() else {
        return error_response(
            StatusCode::CONFLICT,
            "another CPU profile is already running",
        );
    };
    let seconds = query
        .seconds
        .unwrap_or(DEFAULT_PROFILE_SECONDS)
        .clamp(1, MAX_PROFILE_SECONDS);
    let frequency = query
        .frequency
        .unwrap_or(DEFAULT_SAMPLE_FREQUENCY)
        .clamp(1, MAX_SAMPLE_FREQUENCY);

    match tokio::task::spawn_blocking(move || {
        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(frequency)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
            .map_err(|error| error.to_string())?;
        std::thread::sleep(Duration::from_secs(seconds));
        let report = guard.report().build().map_err(|error| error.to_string())?;
        let mut svg = Vec::new();
        report
            .flamegraph(&mut svg)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(svg)
    })
    .await
    {
        Ok(Ok(svg)) => svg_response(svg, "cpu-flamegraph.svg"),
        Ok(Err(error)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, error),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[cfg(target_os = "linux")]
async fn heap_flamegraph() -> Response {
    let Some(profiler) = jemalloc_pprof::PROF_CTL.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "jemalloc heap profiling is unavailable",
        );
    };
    let mut profiler = profiler.lock().await;
    if !profiler.activated()
        && let Err(error) = profiler.activate()
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    match profiler.dump_flamegraph() {
        Ok(svg) => svg_response(svg, "heap-flamegraph.svg"),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

fn svg_response(svg: Vec<u8>, filename: &str) -> Response {
    let mut response = Response::new(Body::from(svg));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("image/svg+xml"));
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("inline; filename=\"{filename}\""))
            .expect("profile filename is a valid header value"),
    );
    response
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message.into()))
        .expect("static profiling error response is valid")
}
