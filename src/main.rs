mod cli;
mod cmd;
mod tui;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Commands, OutputFormat};

fn main() -> Result<()> {
    env_logger::init();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = raw.first() {
        let known = ["run", "inspect", "validate"];
        if first.ends_with(".wasm") && !known.contains(&first.as_str()) {
            tui::launch_tui(&raw);
        }
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            wasm_path,
            once: _,
            env,
            params_json,
            params_file,
            log_channel,
            events,
            verbose,
        } => cmd::cmd_run(cmd::RunOptions {
            wasm_path: &wasm_path,
            env: &env,
            params_json: params_json.as_deref(),
            params_file: params_file.as_deref(),
            log_channel,
            events_mode: events,
            verbose,
            output: cli.output.unwrap_or(OutputFormat::Text),
        }),
        Commands::Inspect { wasm_path } => {
            cmd::cmd_inspect(&wasm_path, cli.output.unwrap_or(OutputFormat::Json))
        }
        Commands::Validate { wasm_path } => {
            cmd::cmd_validate(&wasm_path, cli.output.unwrap_or(OutputFormat::Text))
        }
    }
}
