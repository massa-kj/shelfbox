use anyhow::Result;

mod cli;
mod cmd;

fn main() -> Result<()> {
    cli::run()
}
