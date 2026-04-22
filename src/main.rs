mod app_config;
mod cli;
mod cmd;
mod daemon;
mod mission_result;
mod names;
mod tui;

use anyhow::Result;
use clap::Parser;
use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Commands, DaemonAction, LogFormat, OutputFormat, RescueActionArg};

fn main() {
    let cli = Cli::parse();
    let log_format = cli.log_format;
    if let Err(error) = run(cli) {
        print_error(&error, log_format);
        std::process::exit(exit_code(&error));
    }
}

fn run(cli: Cli) -> Result<()> {
    init_tracing(cli.log_format, cli.verbose)?;
    let discovered_config = app_config::load_from_cwd()?;
    let config = prepare_config(discovered_config.as_ref())?;

    match cli.command {
        None => run_without_command(
            cli.wasm.is_some(),
            cli.output,
            discovered_config.as_ref(),
            &config,
        ),
        Some(command) => dispatch_command(command, cli.output, discovered_config.as_ref(), &config),
    }
}

fn prepare_config(
    discovered_config: Option<&app_config::LoadedWorkingDirConfig>,
) -> Result<brrmmmm::config::Config> {
    let mut config = brrmmmm::config::Config::load()?;
    if let Some(discovered_config) = discovered_config {
        discovered_config.apply_runtime_overrides(&mut config);
    }
    Ok(config)
}

fn run_without_command(
    has_wasm_path: bool,
    output: Option<OutputFormat>,
    discovered_config: Option<&app_config::LoadedWorkingDirConfig>,
    config: &brrmmmm::config::Config,
) -> Result<()> {
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if interactive || has_wasm_path {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        tui::launch_tui(&raw, config);
    }

    let Some(discovered_config) = discovered_config else {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        std::process::exit(1);
    };

    let resolved = discovered_config.resolve_run(app_config::RunResolveRequest {
        wasm_path: None,
        env: &[],
        params_json: None,
        params_file: None,
        result_path: None,
        events_override: None,
        log_channel_override: None,
        override_retry_gate: false,
        limits: &config.limits,
    })?;
    execute_run(resolved, output.unwrap_or(OutputFormat::Text), config)
}

fn dispatch_command(
    command: Commands,
    output: Option<OutputFormat>,
    discovered_config: Option<&app_config::LoadedWorkingDirConfig>,
    config: &brrmmmm::config::Config,
) -> Result<()> {
    match command {
        Commands::Run {
            wasm_path,
            once: _,
            env,
            params_json,
            params_file,
            result_path,
            log_channel,
            no_log_channel,
            events,
            no_events,
            override_retry_gate,
        } => {
            let resolved = resolve_run(
                discovered_config,
                app_config::RunResolveRequest {
                    wasm_path: wasm_path.as_deref(),
                    env: &env,
                    params_json: params_json.as_deref(),
                    params_file: params_file.as_deref(),
                    result_path: result_path.as_deref(),
                    events_override: bool_override(events, no_events),
                    log_channel_override: bool_override(log_channel, no_log_channel),
                    override_retry_gate,
                    limits: &config.limits,
                },
            )?;
            execute_run(resolved, output.unwrap_or(OutputFormat::Text), config)
        }
        Commands::Inspect { wasm_path } => cmd::cmd_inspect(
            &resolve_wasm_path(discovered_config, wasm_path.as_deref())?,
            output.unwrap_or(OutputFormat::Json),
            config,
        ),
        Commands::Validate { wasm_path } => cmd::cmd_validate(
            &resolve_wasm_path(discovered_config, wasm_path.as_deref())?,
            output.unwrap_or(OutputFormat::Text),
        ),
        Commands::Rehearse { wasm_path } => cmd::cmd_rehearse(
            &resolve_wasm_path(discovered_config, wasm_path.as_deref())?,
            output.unwrap_or(OutputFormat::Text),
            config,
        ),
        Commands::Explain { record_path } => {
            cmd::cmd_explain(&record_path, output.unwrap_or(OutputFormat::Text))
        }
        Commands::Daemon { action } => dispatch_daemon_action(action),
        Commands::Launch {
            wasm_path,
            name,
            env,
            params_json,
        } => cmd::cmd_launch(
            wasm_path.to_string_lossy().into_owned(),
            name,
            &env,
            params_json,
        ),
        Commands::Missions => cmd::cmd_missions(),
        Commands::Hold { mission, reason } => cmd::cmd_hold(mission, reason),
        Commands::Resume { mission } => cmd::cmd_resume(mission),
        Commands::Abort { mission, reason } => cmd::cmd_abort(mission, reason),
        Commands::Rescue {
            mission,
            action,
            reason,
        } => cmd::cmd_rescue(mission, rescue_action(action), reason),
    }
}

fn dispatch_daemon_action(action: DaemonAction) -> Result<()> {
    match action {
        DaemonAction::Install => cmd::cmd_daemon_install(),
        DaemonAction::Run => cmd::cmd_daemon_run(),
        DaemonAction::Start => cmd::cmd_daemon_start(),
        DaemonAction::Stop => cmd::cmd_daemon_stop(),
        DaemonAction::Restart => cmd::cmd_daemon_restart(),
        DaemonAction::Status => {
            cmd::cmd_daemon_status();
            Ok(())
        }
        DaemonAction::Uninstall => cmd::cmd_daemon_uninstall(),
    }
}

const fn rescue_action(action: RescueActionArg) -> daemon::RescueAction {
    match action {
        RescueActionArg::Retry => daemon::RescueAction::Retry,
        RescueActionArg::Abort => daemon::RescueAction::Abort,
    }
}

fn execute_run(
    resolved: app_config::ResolvedRun,
    output: OutputFormat,
    config: &brrmmmm::config::Config,
) -> Result<()> {
    cmd::cmd_run(cmd::RunOptions {
        wasm_path: &resolved.wasm_path,
        env_vars: resolved.env_vars,
        params_bytes: resolved.params_bytes,
        mission_recorder: resolved.mission_recorder,
        log_channel: resolved.log_channel,
        events_mode: resolved.events_mode,
        override_retry_gate: resolved.override_retry_gate,
        output,
        config,
    })
}

fn resolve_wasm_path(
    discovered_config: Option<&app_config::LoadedWorkingDirConfig>,
    wasm_path: Option<&std::path::Path>,
) -> Result<std::path::PathBuf> {
    discovered_config.map_or_else(
        || {
            wasm_path.map(std::path::Path::to_path_buf).ok_or_else(|| {
                brrmmmm::error::BrrmmmmError::ConfigInvalid("WASM path is required".to_string())
                    .into()
            })
        },
        |discovered_config| discovered_config.resolve_wasm_path(wasm_path),
    )
}

fn resolve_run(
    discovered_config: Option<&app_config::LoadedWorkingDirConfig>,
    request: app_config::RunResolveRequest<'_>,
) -> Result<app_config::ResolvedRun> {
    discovered_config.map_or_else(
        || app_config::resolve_run_without_config(request),
        |discovered_config| discovered_config.resolve_run(request),
    )
}

const fn bool_override(enabled: bool, disabled: bool) -> Option<bool> {
    if enabled {
        Some(true)
    } else if disabled {
        Some(false)
    } else {
        None
    }
}

fn print_error(error: &anyhow::Error, format: LogFormat) {
    match format {
        LogFormat::Text => eprintln!("[brrmmmm] error: {error:#}"),
        LogFormat::Json => {
            let category = error
                .downcast_ref::<brrmmmm::error::BrrmmmmError>()
                .map_or("unexpected", |error| error.category().as_str());
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

fn init_tracing(format: LogFormat, verbose: bool) -> Result<()> {
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
