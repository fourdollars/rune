#![allow(dead_code, unused_imports, unused_variables)]
#![allow(clippy::all)]
use std::env;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

mod agent;
mod cli;
mod concourse;
mod config;
mod embedding;
mod loop_engine;
mod mcp;
mod precommands;
mod provider;
mod sandbox;
#[cfg(feature = "notes")]
mod serve;
mod setup;
mod skills;
mod tools;
mod trace;
#[cfg(feature = "vim")]
mod vim;

use tracing_subscriber::EnvFilter;

fn main() {
    provider::init_crypto_provider();
    // Internal sandbox subcommands — synchronous, must run before tokio.
    // These exec() into the child process and never return.
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "_landlock" => {
                sandbox::landlock::run();
                unreachable!();
            }
            "_seccomp" => {
                sandbox::seccomp::run();
                unreachable!();
            }
            "_net-guard" => {
                sandbox::net_guard::run();
                unreachable!();
            }
            "vim" => {
                #[cfg(feature = "vim")]
                {
                    if let Err(e) = vim::handle_vim_cli(&args) {
                        eprintln!("vim error: {}", e);
                    }
                    return;
                }
                #[cfg(not(feature = "vim"))]
                {
                    eprintln!(
                        "Error: rune was built without '--features vim'. Rebuild with '--features vim'."
                    );
                    std::process::exit(1);
                }
            }
            _ => {}
        }
    }

    // Enter async runtime for everything else
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main());
}

async fn async_main() {
    // Check for subcommands BEFORE clap parses args
    let args: Vec<String> = env::args().collect();

    if args.len() > 1 && args[1] == "init" {
        // Extract --config / -c from remaining args for init
        let config_override = args
            .iter()
            .enumerate()
            .find_map(|(i, a)| {
                if (a == "--config" || a == "-c") && i + 1 < args.len() {
                    Some(args[i + 1].clone())
                } else if let Some(val) = a.strip_prefix("--config=") {
                    Some(val.to_string())
                } else {
                    None
                }
            })
            .or_else(|| env::var("RUNE_CONFIG").ok());
        setup::run_setup(config_override).await;
        return;
    }

    // Handle `rune notes` subcommand
    if args.len() > 1 && args[1] == "notes" {
        #[cfg(feature = "notes")]
        {
            let config_path = args.iter().enumerate().find_map(|(i, a)| {
                if (a == "--config" || a == "-c") && i + 1 < args.len() {
                    Some(args[i + 1].as_str())
                } else {
                    None
                }
            });
            let mut cfg = config::load_without_clap_path(config_path.map(std::path::Path::new))
                .unwrap_or_else(|e| {
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

            // Parse notes-specific args
            // Priority: CLI flags > env vars > [serve] section in rune.toml
            let notes_cfg = &cfg.notes;
            let mut opts = serve::NotesOptions {
                port: notes_cfg.port.unwrap_or(9527),
                bind: notes_cfg
                    .bind
                    .as_deref()
                    .and_then(|b| b.parse().ok())
                    .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            };

            let mut mount_home: Option<String> = None;
            let mut mount_rw: Vec<String> = Vec::new();
            let mut mount_ro: Vec<String> = Vec::new();

            // CLI flags override config file
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--port" | "-p" => {
                        if i + 1 < args.len() {
                            opts.port = args[i + 1].parse().unwrap_or(9527);
                            i += 1;
                        }
                    }
                    "--bind" | "-b" => {
                        if i + 1 < args.len() {
                            opts.bind = args[i + 1]
                                .parse()
                                .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
                            i += 1;
                        }
                    }
                    "-H" | "--mount-home" => {
                        let path = if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                            i += 1;
                            args[i].clone()
                        } else {
                            ".".to_string()
                        };
                        mount_home = Some(path);
                    }
                    "-M" | "--mount-rw" => {
                        let path = if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                            i += 1;
                            args[i].clone()
                        } else {
                            ".".to_string()
                        };
                        mount_rw.push(path);
                    }
                    "-m" | "--mount-ro" => {
                        let path = if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                            i += 1;
                            args[i].clone()
                        } else {
                            ".".to_string()
                        };
                        mount_ro.push(path);
                    }
                    _ => {}
                }
                i += 1;
            }

            config::apply_mount_flags(&mut cfg.policy, mount_home.as_deref(), &mount_rw, &mount_ro);

            serve::run(cfg, opts).await;
            return;
        }
        #[cfg(not(feature = "notes"))]
        {
            eprintln!("error: 'notes' feature is not enabled in this build.");
            eprintln!("To enable it, please compile with: cargo build --features notes");
            std::process::exit(1);
        }
    }

    // Detect Concourse CI mode BEFORE clap parses args (in/out receive positional args)
    let argv0 = env::args().next().unwrap_or_else(|| "rune".into());
    let prog_name = Path::new(&argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or(argv0.clone());

    match prog_name.as_str() {
        "check" => {
            concourse::run(concourse::ConcourseMode::Check).await;
            return;
        }
        "in" => {
            concourse::run(concourse::ConcourseMode::In).await;
            return;
        }
        "out" => {
            concourse::run(concourse::ConcourseMode::Out).await;
            return;
        }
        _ => {}
    }

    let cfg = config::load().unwrap_or_else(|e| {
        eprintln!("warning: config load failed: {}", e);
        config::RuneConfig::default()
    });

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.log_level)),
        )
        .with_target(false)
        .init();

    cli::run().await;
}
