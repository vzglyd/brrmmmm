use anyhow::Result;

use crate::daemon::{
    DaemonClient, DaemonCommand, DaemonMissionSchedulerState, DaemonResponse, socket_path,
};

pub fn cmd_missions() -> Result<()> {
    let sock = socket_path();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut client = DaemonClient::connect(&sock).await?;
        let resp = client.send(&DaemonCommand::Status).await?;
        match resp {
            DaemonResponse::Status { missions } => {
                if missions.is_empty() {
                    println!("no daemon-managed missions");
                } else {
                    println!(
                        "{:<22} {:<18} {:<14} {:<10} {:<10} {:<24} FLAGS",
                        "NAME", "STATE", "PHASE", "LAST RUN", "NEXT", "OUTCOME"
                    );
                    for m in &missions {
                        let mut flags = Vec::new();
                        if m.held {
                            flags.push("HELD");
                        }
                        if m.terminal {
                            flags.push("TERMINAL");
                        }
                        println!(
                            "{:<22} {:<18} {:<14} {:<10} {:<10} {:<24} {}",
                            m.name,
                            format_state(m.state),
                            m.phase,
                            format_time(m.last_run_at_ms.or(m.last_started_at_ms)),
                            format_next(m.next_wake_at_ms),
                            m.last_outcome_status.clone().unwrap_or_else(|| "--".into()),
                            flags.join(" ")
                        );
                    }
                }
                Ok(())
            }
            DaemonResponse::Error { message } => anyhow::bail!("{message}"),
            _ => anyhow::bail!("unexpected response from daemon"),
        }
    })
}

const fn format_state(state: DaemonMissionSchedulerState) -> &'static str {
    match state {
        DaemonMissionSchedulerState::Launching => "launching",
        DaemonMissionSchedulerState::Running => "running",
        DaemonMissionSchedulerState::Scheduled => "scheduled",
        DaemonMissionSchedulerState::Held => "held",
        DaemonMissionSchedulerState::AwaitingChange => "awaiting_change",
        DaemonMissionSchedulerState::AwaitingOperator => "awaiting_operator",
        DaemonMissionSchedulerState::TerminalFailure => "terminal_failure",
        DaemonMissionSchedulerState::Idle => "idle",
    }
}

fn format_time(value: Option<u64>) -> String {
    value
        .map(|ms| {
            let seconds = ms / 1000;
            let minutes = (seconds / 60) % 60;
            let hours = (seconds / 3600) % 24;
            format!("{hours:02}:{minutes:02}")
        })
        .unwrap_or_else(|| "--".into())
}

fn format_next(value: Option<u64>) -> String {
    let Some(wake_ms) = value else {
        return "--".into();
    };
    let now_ms = brrmmmm::events::now_ms();
    let remaining = wake_ms.saturating_sub(now_ms) / 1000;
    if remaining < 60 {
        format!("{remaining}s")
    } else {
        format!("{}m", remaining / 60)
    }
}
