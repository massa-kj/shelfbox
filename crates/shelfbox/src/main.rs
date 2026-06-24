use std::process::ExitCode;

mod cli;
mod commands;

fn main() -> ExitCode {
    match cli::run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:?}");
            ExitCode::from(255)
        }
    }
}
