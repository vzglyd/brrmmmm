use clap::{Parser, Subcommand, ValueEnum, ValueHint};
use std::path::PathBuf;

#[derive(ValueEnum, Clone, Default, PartialEq, Debug)]
pub(crate) enum OutputFormat {
    #[default]
    Text,
    Json,
    Table,
}

#[derive(ValueEnum, Clone, Default, PartialEq, Debug)]
pub(crate) enum LogFormat {
    #[default]
    Text,
    Json,
}

#[derive(Parser)]
#[command(
    name = "brrmmmm",
    about = "Acquisition runtime for portable WASM mission modules",
    after_help = "\
EXAMPLES:
  brrmmmm mission-module.wasm       # launches TUI
  brrmmmm                          # runs ./brrmmmm.toml when present
  brrmmmm run      mission-module.wasm --once
  brrmmmm run      --once --result-path mission.json
  brrmmmm run      mission-module.wasm --once --output json
  brrmmmm inspect  mission-module.wasm --output table
  brrmmmm validate mission-module.wasm
  brrmmmm validate mission-module.wasm --output table
  brrmmmm rehearse mission-module.wasm
  brrmmmm explain  mission.json",
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

    /// Diagnostic log format written to stderr.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Text)]
    pub(crate) log_format: LogFormat,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    /// Path to the mission-module `.wasm` file (launches TUI if provided without a subcommand)
    #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
    pub(crate) wasm: Option<PathBuf>,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Run a mission-module WASM module
    Run {
        /// Path to the mission-module `.wasm` file. Falls back to mission.wasm in ./brrmmmm.toml.
        #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
        wasm_path: Option<PathBuf>,

        /// Run a single acquisition and exit (default and currently the only mode)
        #[arg(long)]
        once: bool,

        /// Set environment variable (KEY=VALUE)
        #[arg(short = 'e', long, value_name = "KEY=VALUE", value_parser = parse_key_val)]
        env: Vec<String>,

        /// JSON object exposed through the mission-module params_len/params_read imports
        #[arg(short = 'j', long, conflicts_with = "params_file")]
        params_json: Option<String>,

        /// Path to a JSON file exposed through the mission-module params_len/params_read imports
        #[arg(short = 'f', long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        params_file: Option<PathBuf>,

        /// Path to a durable mission-result JSON file
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        result_path: Option<PathBuf>,

        /// Log channel pushes to stderr
        #[arg(long)]
        log_channel: bool,

        /// Disable log channel pushes configured by default
        #[arg(long = "no-log-channel", conflicts_with = "log_channel")]
        no_log_channel: bool,

        /// Emit structured NDJSON event stream to stdout (for TUI subprocess mode)
        #[arg(long)]
        events: bool,

        /// Disable structured NDJSON events configured by default
        #[arg(long = "no-events", conflicts_with = "events")]
        no_events: bool,

        /// Allow one attempt even when the repeat-failure gate requires changed conditions
        #[arg(long)]
        override_retry_gate: bool,
    },

    /// Inspect a mission-module WASM module and print its contract
    Inspect {
        /// Path to the mission-module `.wasm` file. Falls back to mission.wasm in ./brrmmmm.toml.
        #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
        wasm_path: Option<PathBuf>,
    },

    /// Validate that a mission-module WASM module loads correctly
    Validate {
        /// Path to the mission-module `.wasm` file. Falls back to mission.wasm in ./brrmmmm.toml.
        #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
        wasm_path: Option<PathBuf>,
    },

    /// Rehearse host decision paths without launching a live mission attempt
    Rehearse {
        /// Path to the mission-module `.wasm` file. Falls back to mission.wasm in ./brrmmmm.toml.
        #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
        wasm_path: Option<PathBuf>,
    },

    /// Explain a durable mission record
    Explain {
        /// Path to the mission record JSON file.
        #[arg(value_name = "PATH", value_hint = ValueHint::FilePath)]
        record_path: PathBuf,
    },
}

fn parse_key_val(s: &str) -> Result<String, String> {
    if s.contains('=') {
        Ok(s.to_string())
    } else {
        Err(format!("invalid KEY=VALUE: no `=` found in `{s}`"))
    }
}
