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
mod mcp;
mod precommands;
mod provider;
mod sandbox;
mod serve;
mod setup;
mod skills;
mod tools;
mod trace;

use tracing_subscriber::EnvFilter;

fn main() {
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
        let cfg = config::load_without_clap().unwrap_or_else(|e| {
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
            user_token: notes_cfg.user_token.clone().or_else(|| notes_cfg.token.clone()),
            admin_token: notes_cfg.admin_token.clone(),
            guest_token: notes_cfg.guest_token.clone(),
        };

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
                "--token" | "-t" => {
                    if i + 1 < args.len() {
                        opts.user_token = Some(args[i + 1].clone());
                        i += 1;
                    }
                }
                "--admin-token" | "-a" => {
                    if i + 1 < args.len() {
                        opts.admin_token = Some(args[i + 1].clone());
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }

        // Env var override (higher than config, lower than CLI flags)
        if opts.user_token.is_none() {
            if let Ok(t) = env::var("RUNE_NOTES_USER_TOKEN") {
                if !t.is_empty() {
                    opts.user_token = Some(t);
                }
            }
        }
        if opts.admin_token.is_none() {
            if let Ok(t) = env::var("RUNE_NOTES_ADMIN_TOKEN") {
                if !t.is_empty() {
                    opts.admin_token = Some(t);
                }
            }
        }

        serve::run(cfg, opts).await;
        return;
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
