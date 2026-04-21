use anyhow::Result;

use crate::daemon::{DaemonClient, DaemonCommand, DaemonResponse, socket_path};

pub(crate) fn cmd_missions() -> Result<()> {
    let sock = socket_path();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut client = DaemonClient::connect(&sock).await?;
        let resp = client.send(&DaemonCommand::Status).await?;
        match resp {
            DaemonResponse::Status { missions } => {
                if missions.is_empty() {
                    println!("no active missions");
                } else {
                    println!("{:<22} {:<14} {:>8}  FLAGS", "NAME", "PHASE", "CYCLES");
                    for m in &missions {
                        let mut flags = Vec::new();
                        if m.held {
                            flags.push("HELD");
                        }
                        if m.terminal {
                            flags.push("TERMINAL");
                        }
                        println!(
                            "{:<22} {:<14} {:>8}  {}",
                            m.name,
                            m.phase,
                            m.cycles,
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
