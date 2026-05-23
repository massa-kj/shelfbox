use std::process::ExitCode;

mod cli;
mod cmd;

fn main() -> ExitCode {
    match cli::run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:?}");
            ExitCode::from(255)
        }
    }
}
