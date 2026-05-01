pub fn run() {
    // Use simple ANSI escapes for color so we don't add external deps
    const GREEN: &str = "\x1b[32m";
    const CYAN: &str = "\x1b[36m";
    const RESET: &str = "\x1b[0m";

    println!("{}=== rune CLI ==={}", GREEN, RESET);
    println!("{}Welcome to the rune CLI!{}", CYAN, RESET);
    println!("Type 'help' for commands, 'exit' or Ctrl-D to quit.");

    use std::io::{self, BufRead};

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        match line {
            Ok(l) => {
                let cmd = l.trim();
                if cmd == "exit" || cmd == "quit" {
                    println!("Goodbye!");
                    break;
                }
                if cmd == "help" {
                    println!("Available (placeholder): help, exit");
                    continue;
                }
                println!("You typed: {}", l);
            }
            Err(_) => break,
        }
    }
}
