use clap::{Parser, Subcommand, ValueEnum, ValueHint};
use std::path::PathBuf;

#[derive(ValueEnum, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    Table,
}

#[derive(ValueEnum, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum RescueActionArg {
    Retry,
    Abort,
}

#[derive(Parser)]
#[command(
    name = "brrmmmm",
    about = "Sidecar mission runner for portable WASM mission modules",
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
  brrmmmm explain  mission.json
  brrmmmm daemon install
  brrmmmm daemon start
  brrmmmm launch   mission-module.wasm --name vrx64-crypto
  brrmmmm missions
  brrmmmm hold     solar-wind --reason \"maintenance window\"
  brrmmmm resume   solar-wind
  brrmmmm abort    solar-wind --reason \"shutting down\"
  brrmmmm rescue   solar-wind --action retry --reason \"fixed API key\"

NOTES:
  Watch the mission JSON files for downstream integrations.
  Daemon missions persist ~/.brrmmmm/missions/<mission_name>/<mission_name>.status.json while running.
  Daemon missions persist ~/.brrmmmm/missions/<mission_name>/<mission_name>.out.json for the latest finalized attempt.",
    version
)]
pub struct Cli {
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
pub enum Commands {
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

        /// JSON object exposed through the mission-module `params_len/params_read` imports
        #[arg(short = 'j', long, conflicts_with = "params_file")]
        params_json: Option<String>,

        /// Path to a JSON file exposed through the mission-module `params_len/params_read` imports
        #[arg(short = 'f', long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        params_file: Option<PathBuf>,

        /// Path to a durable final mission-result JSON file watched by downstream consumers
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

    /// Manage the brrmmmm mission daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Launch a mission in the daemon
    Launch {
        /// Path to the mission-module `.wasm` file
        #[arg(value_name = "WASM", value_hint = ValueHint::FilePath)]
        wasm_path: PathBuf,

        /// Stable daemon mission name used for ~/.brrmmmm/missions/<name>/...
        #[arg(long)]
        name: String,

        /// Set environment variable (KEY=VALUE)
        #[arg(short = 'e', long, value_name = "KEY=VALUE", value_parser = parse_key_val)]
        env: Vec<String>,

        /// JSON params object
        #[arg(short = 'j', long)]
        params_json: Option<String>,
    },

    /// List all missions in the daemon
    Missions,

    /// Pause a running mission
    Hold {
        /// Mission name (e.g. solar-wind)
        mission: String,

        /// Reason for holding the mission
        #[arg(long)]
        reason: String,
    },

    /// Resume a held mission
    Resume {
        /// Mission name
        mission: String,
    },

    /// Abort a running mission permanently
    Abort {
        /// Mission name
        mission: String,

        /// Reason for aborting
        #[arg(long)]
        reason: String,
    },

    /// Rescue a mission that is stuck or gate-blocked
    Rescue {
        /// Mission name
        mission: String,

        /// retry: clear gate and restart; abort: terminate permanently
        #[arg(long, value_enum)]
        action: RescueActionArg,

        /// Reason for the rescue
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand, Clone, Copy, Debug)]
pub enum DaemonAction {
    /// Generate and enable the systemd/launchd service unit
    Install,
    /// Run the daemon in the foreground (called by the service manager)
    Run,
    /// Start the daemon via the service manager
    Start,
    /// Stop the daemon via the service manager
    Stop,
    /// Restart the daemon via the service manager
    Restart,
    /// Show daemon status
    Status,
    /// Disable and remove the service unit
    Uninstall,
}

fn parse_key_val(s: &str) -> Result<String, String> {
    if s.contains('=') {
        Ok(s.to_string())
    } else {
        Err(format!("invalid KEY=VALUE: no `=` found in `{s}`"))
    }
}
