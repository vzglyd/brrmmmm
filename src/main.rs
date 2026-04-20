mod cli;
mod cmd;
mod tui;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Commands, LogFormat, OutputFormat};

fn main() {
    let cli = Cli::parse();
    let log_format = cli.log_format.clone();
    if let Err(error) = run(cli) {
        print_error(&error, &log_format);
        std::process::exit(exit_code(&error));
    }
}

fn run(cli: Cli) -> Result<()> {
    init_tracing(&cli.log_format, cli.verbose)?;
    let config = brrmmmm::config::Config::load()?;

    match (cli.command, cli.wasm) {
        (None, Some(_wasm)) => {
            let raw: Vec<String> = std::env::args().skip(1).collect();
            tui::launch_tui(&raw, &config);
        }
        (None, None) => {
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
                config: &config,
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

fn print_error(error: &anyhow::Error, format: &LogFormat) {
    match format {
        LogFormat::Text => eprintln!("[brrmmmm] error: {error:#}"),
        LogFormat::Json => {
            let category = error
                .downcast_ref::<brrmmmm::error::BrrmmmmError>()
                .map(|error| error.category().as_str())
                .unwrap_or("unexpected");
            let event = serde_json::json!({
                "level": "error",
                "target": "brrmmmm",
                "category": category,
                "message": format!("{error:#}"),
            });
            eprintln!("{event}");
        }
    }
}

fn init_tracing(format: &LogFormat, verbose: bool) -> Result<()> {
    let default_level = if verbose { "debug" } else { "warn" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    match format {
        LogFormat::Text => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init()
            .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?,
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init()
            .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?,
    }
    Ok(())
}

fn exit_code(error: &anyhow::Error) -> i32 {
    if let Some(error) = error.downcast_ref::<brrmmmm::error::BrrmmmmError>() {
        return error.exit_code();
    }
    1
}
