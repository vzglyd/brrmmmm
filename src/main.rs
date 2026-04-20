mod cli;
mod cmd;
mod tui;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Commands, OutputFormat};

fn main() -> Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    match (cli.command, cli.wasm) {
        (None, Some(_wasm)) => {
            // Re-collect args for TUI launch (including global flags)
            let raw: Vec<String> = std::env::args().skip(1).collect();
            tui::launch_tui(&raw);
        }
        (None, None) => {
            // This case should be handled by clap (required command or positional)
            // but just in case:
            use clap::CommandFactory;
            Cli::command().print_help()?;
            std::process::exit(1);
        }
        (Some(command), _) => match command {
            Commands::Run {
                wasm_path,
                once: _,
                env,
                params_json,
                params_file,
                log_channel,
                events,
            } => cmd::cmd_run(cmd::RunOptions {
                wasm_path: &wasm_path,
                env: &env,
                params_json: params_json.as_deref(),
                params_file: params_file.as_deref(),
                log_channel,
                events_mode: events,
                verbose: cli.verbose,
                output: cli.output.unwrap_or(OutputFormat::Text),
            }),
            Commands::Inspect { wasm_path } => {
                cmd::cmd_inspect(&wasm_path, cli.output.unwrap_or(OutputFormat::Json))
            }
            Commands::Validate { wasm_path } => {
                cmd::cmd_validate(&wasm_path, cli.output.unwrap_or(OutputFormat::Text))
            }
        },
    }
}
