use clap::{Parser, Subcommand, ValueEnum, ValueHint};
use std::path::PathBuf;

#[derive(ValueEnum, Clone, Default, PartialEq, Debug)]
pub(crate) enum OutputFormat {
    #[default]
    Text,
    Json,
    Table,
}

#[derive(Parser)]
#[command(
    name = "brrmmmm",
    about = "Synchronous acquisition runtime for portable WASM sidecars",
    after_help = "\
EXAMPLES:
  brrmmmm sidecar.wasm              # launches TUI
  brrmmmm run      sidecar.wasm --once
  brrmmmm run      sidecar.wasm --once --output json
  brrmmmm inspect  sidecar.wasm --output table
  brrmmmm validate sidecar.wasm
  brrmmmm validate sidecar.wasm --output table",
    version
)]
pub(crate) struct Cli {
    /// Output format: json, text, or table.
    /// Default: json for inspect, text for validate and run.
    #[arg(long, global = true, value_enum)]
    pub(crate) output: Option<OutputFormat>,

    /// Verbose output
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    /// Path to the sidecar .wasm file (launches TUI if provided without a subcommand)
    #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
    pub(crate) wasm: Option<PathBuf>,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Run a sidecar WASM module
    Run {
        /// Path to the sidecar .wasm file
        #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
        wasm_path: PathBuf,

        /// Run a single acquisition and exit (default and currently the only mode)
        #[arg(long)]
        once: bool,

        /// Set environment variable (KEY=VALUE)
        #[arg(short = 'e', long, value_name = "KEY=VALUE", value_parser = parse_key_val)]
        env: Vec<String>,

        /// JSON object exposed through the sidecar params_len/params_read imports
        #[arg(short = 'j', long, conflicts_with = "params_file")]
        params_json: Option<String>,

        /// Path to a JSON file exposed through the sidecar params_len/params_read imports
        #[arg(short = 'f', long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        params_file: Option<PathBuf>,

        /// Log channel pushes to stderr
        #[arg(long)]
        log_channel: bool,

        /// Emit structured NDJSON event stream to stdout (for TUI subprocess mode)
        #[arg(long)]
        events: bool,
    },

    /// Inspect a sidecar WASM module and print its contract
    Inspect {
        /// Path to the sidecar .wasm file
        #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
        wasm_path: PathBuf,
    },

    /// Validate that a sidecar WASM module loads correctly
    Validate {
        /// Path to the sidecar .wasm file
        #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
        wasm_path: PathBuf,
    },
}

fn parse_key_val(s: &str) -> Result<String, String> {
    if s.contains('=') {
        Ok(s.to_string())
    } else {
        Err(format!("invalid KEY=VALUE: no `=` found in `{s}`"))
    }
}
