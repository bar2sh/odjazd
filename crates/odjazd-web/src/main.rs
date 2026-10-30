//! The odjazd web app and its JSON API.
//!
//! One process does it all: it keeps Warsaw's timetable in memory (downloading a fresh one
//! every night), follows the city's live vehicle positions, and serves the API described
//! in `API.md` along with the web app in `web/`.

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use tower_http::compression::CompressionLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod api;
mod assets;
mod live;
mod state;

/// Warsaw's timetable: ZTM Warszawa data, converted to GTFS by Mikołaj Kuranowski.
const WARSAW_FEED: &str = "https://mkuran.pl/gtfs/warsaw.zip";

/// Where Warsaw's buses and trams are right now (GTFS-Realtime), from the City Hall's
/// open data, matched to the timetable's trips by Mikołaj Kuranowski.
const WARSAW_VEHICLES: &str = "https://mkuran.pl/gtfs/warsaw/vehicles.pb";

/// How odjazd introduces itself to the servers it downloads from.
const USER_AGENT: &str = concat!(
    "odjazd-web/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/bar2sh/odjazd)"
);

/// Serves the odjazd web app: the poles nearest to you, live departures, and which one
/// to walk to.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Address to listen on; 0.0.0.0 to accept connections from other machines
    #[arg(long, env = "ODJAZD_HOST", default_value = "127.0.0.1")]
    host: IpAddr,

    /// Port to listen on
    #[arg(long, env = "PORT", default_value_t = 8080)]
    port: u16,

    /// Where to keep the timetable [default: the cache directory the odjazd command uses]
    #[arg(long, env = "ODJAZD_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// GTFS feed to serve
    #[arg(long, env = "ODJAZD_FEED_URL", default_value = WARSAW_FEED)]
    feed_url: String,

    /// GTFS-Realtime vehicle positions to follow
    #[arg(long, env = "ODJAZD_REALTIME_URL", default_value = WARSAW_VEHICLES)]
    realtime_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();
    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => dirs::cache_dir()
            .context("cannot find a cache directory; set ODJAZD_DATA_DIR")?
            .join("odjazd"),
    };

    let state = state::AppState::default();
    tokio::spawn(state::keep_timetable(
        state.clone(),
        data_dir.join("feed"),
        args.feed_url,
    ));
    tokio::spawn(live::follow(state.clone(), args.realtime_url));

    let app = api::router(state)
        .fallback(assets::serve)
        .layer(CompressionLayer::new());
    let address = SocketAddr::new(args.host, args.port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen on {address}"))?;
    info!("listening on http://{address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// Resolves on Ctrl-C, or when the system asks the server to stop.
async fn shutdown() {
    let interrupt = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            terminate.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    info!("shutting down");
}
