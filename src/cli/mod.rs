pub async fn run() {
    const GREEN: &str = "\x1b[32m";
    const CYAN: &str = "\x1b[36m";
    const RESET: &str = "\x1b[0m";

    println!("{}=== rune CLI ==={}", GREEN, RESET);
    println!("{}Welcome to the rune CLI!{}", CYAN, RESET);
    println!("Type 'help' for commands, 'exit' or Ctrl-D to quit.");

    let config = crate::config::load().unwrap_or_default();
    let mut agent = crate::agent::Agent::new(config);
    agent.set_system_prompt("You are an AI agent. Respond concisely.");

    use tokio::io::{self, AsyncBufReadExt};
    let stdin = io::stdin();
    let reader = io::BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let cmd = line.trim().to_string();
        if cmd == "exit" || cmd == "quit" {
            println!("Goodbye!");
            break;
        }
        if cmd == "help" {
            println!("Available: help, exit, run <text>");
            continue;
        }
        if let Some(input) = cmd.strip_prefix("run ") {
            match agent.run(input).await {
                crate::agent::StopReason::FinalAnswer(ans) => println!("Final answer: {}", ans),
                crate::agent::StopReason::MaxSteps => println!("Stopped: max steps reached"),
                crate::agent::StopReason::TokenBudgetExhausted => println!("Stopped: token budget exhausted"),
                crate::agent::StopReason::Error(e) => println!("Error: {}", e),
                crate::agent::StopReason::UserInterrupt => println!("Interrupted by user"),
            }
            continue;
        }
        println!("Unknown command: {}", cmd);
    }
}
