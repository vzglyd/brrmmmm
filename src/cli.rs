use clap::{Parser, Subcommand, ValueEnum};

#[derive(ValueEnum, Clone, Default, PartialEq)]
pub(crate) enum OutputFormat {
    #[default]
    Text,
    Json,
    Table,
}

#[derive(Parser)]
#[command(
    name = "brrmmmm",
    about = "Standalone sidecar runner for VZGLYD sidecar WASM modules",
    after_help = "\
EXAMPLES:
  brrmmmm validate sidecar.wasm
  brrmmmm validate sidecar.wasm --output table
  brrmmmm inspect  sidecar.wasm --output table
  brrmmmm run      sidecar.wasm --once
  brrmmmm run      sidecar.wasm --once --output json
  brrmmmm          sidecar.wasm              # launches TUI",
    version
)]
pub(crate) struct Cli {
    /// Output format: json, text, or table.
    /// Default: json for inspect, text for validate and run.
    #[arg(long, global = true, value_enum)]
    pub(crate) output: Option<OutputFormat>,

    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Run a sidecar WASM module
    Run {
        /// Path to the sidecar .wasm file
        wasm_path: String,

        /// Run a single fetch iteration and exit
        #[arg(long)]
        once: bool,

        /// Poll interval in seconds (default: 60)
        #[arg(long, default_value_t = 60)]
        interval: u64,

        /// Set environment variable (KEY=VALUE)
        #[arg(long, value_name = "KEY=VALUE")]
        env: Vec<String>,

        /// JSON object passed to the sidecar configure buffer
        #[arg(long, conflicts_with = "params_file")]
        params_json: Option<String>,

        /// Path to a JSON file passed to the sidecar configure buffer
        #[arg(long, value_name = "PATH")]
        params_file: Option<String>,

        /// Log channel pushes to stderr
        #[arg(long)]
        log_channel: bool,

        /// Emit structured NDJSON event stream to stdout (for TUI subprocess mode)
        #[arg(long)]
        events: bool,

        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Inspect a sidecar WASM module and print its contract
    Inspect {
        /// Path to the sidecar .wasm file
        wasm_path: String,
    },

    /// Validate that a sidecar WASM module loads correctly
    Validate {
        /// Path to the sidecar .wasm file
        wasm_path: String,
    },
}
