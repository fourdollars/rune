use std::env;
use std::path::Path;

mod agent;
mod cli;
mod concourse;
mod config;
mod mcp;
mod precommands;
mod provider;
mod sandbox;
mod setup;
mod skills;
mod tools;
mod trace;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Check for subcommands BEFORE clap parses args
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--json") {
        std::env::set_var("RUNE_JSON_OUTPUT", "1");
    }
    if args.len() > 1 && args[1] == "init" {
        setup::run_setup().await;
        return;
    }

    let cfg = config::load().unwrap_or_else(|e| {
        eprintln!("warning: config load failed: {}", e);
        config::RuneConfig::default()
    });

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&cfg.log_level)),
        )
        .with_target(false)
        .init();

    let argv0 = env::args().next().unwrap_or_else(|| "rune".into());
    let prog_name = Path::new(&argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or(argv0);

    match prog_name.as_str() {
        "check" => concourse::run(concourse::ConcourseMode::Check),
        "in" => concourse::run(concourse::ConcourseMode::In),
        "out" => concourse::run(concourse::ConcourseMode::Out),
        _ => cli::run().await,
    }
}
