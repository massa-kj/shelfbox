use std::process::ExitCode;

use shelfbox_core::error::AppError;

mod cli;
mod commands;

fn main() -> ExitCode {
    match cli::run() {
        Ok(code) => code,
        Err(e) => {
            if let Some(AppError::MutationDurabilityUnavailable {
                operation,
                platform,
                ..
            }) = e.chain().find_map(|cause| cause.downcast_ref::<AppError>())
            {
                eprintln!(
                    "error: {operation} requires crash-safe directory durability, which is unavailable on {platform}."
                );
                eprintln!();
                eprintln!("To allow reduced-guarantee updates on this machine:");
                eprintln!("  shelfbox config set mutation_durability best-effort");
                eprintln!();
                eprintln!(
                    "best-effort does not guarantee complete recovery after power loss or forced termination."
                );
            } else {
                eprintln!("error: {e:?}");
            }
            ExitCode::from(255)
        }
    }
}
