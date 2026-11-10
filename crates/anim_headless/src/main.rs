mod cli;
use clap::Parser;

fn main() {
    match cli::Cli::parse().run() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
