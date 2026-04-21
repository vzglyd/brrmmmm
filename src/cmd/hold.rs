use anyhow::Result;

use crate::daemon::{DaemonClient, DaemonCommand, DaemonResponse, RescueAction, socket_path};

pub(crate) fn cmd_hold(mission: String, reason: String) -> Result<()> {
    send_simple(DaemonCommand::Hold { mission, reason })
}

pub(crate) fn cmd_resume(mission: String) -> Result<()> {
    send_simple(DaemonCommand::Resume { mission })
}

pub(crate) fn cmd_abort(mission: String, reason: String) -> Result<()> {
    send_simple(DaemonCommand::Abort { mission, reason })
}

pub(crate) fn cmd_rescue(mission: String, action: RescueAction, reason: String) -> Result<()> {
    send_simple(DaemonCommand::Rescue {
        mission,
        action,
        reason,
    })
}

fn send_simple(cmd: DaemonCommand) -> Result<()> {
    let sock = socket_path();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut client = DaemonClient::connect(&sock).await?;
        let resp = client.send(&cmd).await?;
        match resp {
            DaemonResponse::Ok { mission } => {
                println!("{mission}: ok");
                Ok(())
            }
            DaemonResponse::Error { message } => anyhow::bail!("{message}"),
            _ => anyhow::bail!("unexpected response from daemon"),
        }
    })
}
