mod app;
mod settings;
mod transport;
mod ui;

use anyhow::{Context, Result};
use std::env;
use std::sync::Arc;
use tokio::runtime::Builder;

fn main() -> Result<()> {
    let runtime = Arc::new(
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("failed to build tokio runtime")?,
    );

    let args: Vec<String> = env::args().collect();
    if matches!(args.get(1).map(String::as_str), Some("gui")) {
        return app::run_gui(runtime);
    }
    if args.len() > 1 {
        return runtime.block_on(app::run_cli(&args));
    }

    app::run_gui(runtime)
}
