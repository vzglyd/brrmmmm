use anyhow::Result;

use crate::daemon::{
    self, daemon_install, daemon_restart, daemon_start, daemon_status, daemon_stop,
    daemon_uninstall,
};

pub(crate) fn cmd_daemon_run() -> Result<()> {
    tokio::runtime::Runtime::new()?.block_on(daemon::run())
}

pub(crate) fn cmd_daemon_install() -> Result<()> {
    daemon_install()
}

pub(crate) fn cmd_daemon_start() -> Result<()> {
    daemon_start()
}

pub(crate) fn cmd_daemon_stop() -> Result<()> {
    daemon_stop()
}

pub(crate) fn cmd_daemon_restart() -> Result<()> {
    daemon_restart()
}

pub(crate) fn cmd_daemon_status() -> Result<()> {
    daemon_status()
}

pub(crate) fn cmd_daemon_uninstall() -> Result<()> {
    daemon_uninstall()
}
