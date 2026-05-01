pub enum ConcourseMode {
    Check,
    In,
    Out,
}

pub fn run(mode: ConcourseMode) {
    use std::io::{self, Read};

    // read all stdin (Concourse provides JSON on stdin)
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("Failed to read stdin: {}", e);
        return;
    }

    match mode {
        ConcourseMode::Check => {
            // placeholder: normally would parse request and emit version info
            println!("Concourse mode: CHECK");
            println!("Received {} bytes from stdin", input.len());
        }
        ConcourseMode::In => {
            println!("Concourse mode: IN");
            println!("Received {} bytes from stdin", input.len());
        }
        ConcourseMode::Out => {
            println!("Concourse mode: OUT");
            println!("Received {} bytes from stdin", input.len());
        }
    }
}
