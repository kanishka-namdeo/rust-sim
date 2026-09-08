//! fleet-catalog: the `:8300` mission-catalog + replay server.
//!
//! Binary entry point for ADR-0027. Composes the `gcs::server` module
//! with a filesystem `Store` and binds to `:8300`.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

use fleet_mission::gcs::server::{serve, AppState};
use fleet_mission::gcs::store::Store;

/// fleet-catalog: the RustSim GCS mission-catalog + replay server.
#[derive(Parser, Debug)]
#[command(name = "fleet-catalog", version, about)]
struct Args {
    /// Port to bind (default 8300, per GCS_SPEC.md §4.3).
    #[arg(long, default_value_t = 8300)]
    port: u16,

    /// Catalog directory (default /tmp/rustsim-catalog; set via env RSIM_CATALOG_DIR).
    #[arg(long)]
    catalog_dir: Option<PathBuf>,

    /// Fleet manager base URL (default http://127.0.0.1:8400; set via env RSIM_FLEET_URL).
    #[arg(long, default_value = "http://127.0.0.1:8400")]
    fleet_url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let catalog_dir = args.catalog_dir
        .or_else(|| std::env::var("RSIM_CATALOG_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| std::env::temp_dir().join("rustsim-catalog"));

    let fleet_url = args.fleet_url.clone();
    let store = Store::new(&catalog_dir)?;
    let state = Arc::new(AppState {
        store,
        fleet_base_url: fleet_url.clone(),
    });

    println!(
        "[fleet-catalog] catalog dir: {}",
        catalog_dir.display()
    );
    println!("[fleet-catalog] fleet manager: {}", fleet_url);

    serve(state, args.port).await?;

    Ok(())
}
