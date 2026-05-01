use std::env;
use std::path::Path;

mod cli;
mod concourse;
mod config;

fn main() {
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
        _ => cli::run(),
    }
}
