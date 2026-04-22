use anyhow::Result;

use crate::daemon::{
    self, daemon_install, daemon_restart, daemon_start, daemon_status, daemon_stop,
    daemon_uninstall, is_service_not_installed,
};

pub fn cmd_daemon_run() -> Result<()> {
    tokio::runtime::Runtime::new()?.block_on(daemon::run())
}

pub fn cmd_daemon_install() -> Result<()> {
    daemon_install()
}

pub fn cmd_daemon_start() -> Result<()> {
    warn_if_service_missing(daemon_start(), "start")
}

pub fn cmd_daemon_stop() -> Result<()> {
    warn_if_service_missing(daemon_stop(), "stop")
}

pub fn cmd_daemon_restart() -> Result<()> {
    warn_if_service_missing(daemon_restart(), "restart")
}

pub fn cmd_daemon_status() {
    daemon_status();
}

pub fn cmd_daemon_uninstall() -> Result<()> {
    daemon_uninstall()
}

fn warn_if_service_missing(result: Result<()>, action: &str) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if is_service_not_installed(&error) => {
            eprintln!("[brrmmmm] warning: daemon service is not installed");
            eprintln!("[brrmmmm] run `brrmmmm daemon install` before `brrmmmm daemon {action}`");
            Ok(())
        }
        Err(error) => Err(error),
    }
}
