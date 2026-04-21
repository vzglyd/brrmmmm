use anyhow::Result;

pub(crate) fn daemon_install() -> Result<()> {
    platform::install()
}

pub(crate) fn daemon_start() -> Result<()> {
    platform::start()
}

pub(crate) fn daemon_stop() -> Result<()> {
    platform::stop()
}

pub(crate) fn daemon_restart() -> Result<()> {
    platform::restart()
}

pub(crate) fn daemon_status() -> Result<()> {
    platform::status()?;
    probe_socket();
    Ok(())
}

pub(crate) fn daemon_uninstall() -> Result<()> {
    platform::uninstall()
}

fn probe_socket() {
    let sock = super::socket_path();
    if !sock.exists() {
        println!("socket: not found (daemon not running?)");
        return;
    }
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(_) => return,
    };
    match rt.block_on(async {
        let mut client = super::client::DaemonClient::connect(&sock).await?;
        client.send(&super::protocol::Command::Ping).await
    }) {
        Ok(super::protocol::Response::Pong) => println!("socket: responding"),
        Ok(_) => println!("socket: unexpected response"),
        Err(e) => println!("socket: {e}"),
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use anyhow::Result;

    pub(super) fn install() -> Result<()> {
        let exe = std::env::current_exe()?;
        let unit_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("no home directory"))?
            .join(".config/systemd/user");
        std::fs::create_dir_all(&unit_dir)?;

        let unit = format!(
            "[Unit]\n\
             Description=brrmmmm mission runtime daemon\n\
             After=network-online.target\n\
             Wants=network-online.target\n\
             StartLimitBurst=5\n\
             StartLimitIntervalSec=60\n\
             \n\
             [Service]\n\
             ExecStart={exe} daemon run\n\
             Restart=always\n\
             RestartSec=5\n\
             RestartPreventExitStatus=78\n\
             TimeoutStopSec=30\n\
             TimeoutStartSec=30\n\
             SuccessExitStatus=0 143\n\
             KillMode=control-group\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            exe = exe.display()
        );

        let unit_path = unit_dir.join("brrmmmm.service");
        std::fs::write(&unit_path, unit)?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "brrmmmm"])?;
        println!("installed {}", unit_path.display());
        println!("run `brrmmmm daemon start` to start the daemon");
        Ok(())
    }

    pub(super) fn start() -> Result<()> {
        systemctl(&["start", "brrmmmm"])?;
        println!("brrmmmm daemon started");
        Ok(())
    }

    pub(super) fn stop() -> Result<()> {
        systemctl(&["stop", "brrmmmm"])?;
        println!("brrmmmm daemon stopped");
        Ok(())
    }

    pub(super) fn restart() -> Result<()> {
        systemctl(&["restart", "brrmmmm"])?;
        println!("brrmmmm daemon restarted");
        Ok(())
    }

    pub(super) fn status() -> Result<()> {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "status", "brrmmmm"])
            .status();
        Ok(())
    }

    pub(super) fn uninstall() -> Result<()> {
        systemctl(&["stop", "brrmmmm"]).ok();
        systemctl(&["disable", "brrmmmm"]).ok();
        if let Some(path) = dirs::home_dir().map(|h| h.join(".config/systemd/user/brrmmmm.service"))
            && path.exists()
        {
            std::fs::remove_file(&path)?;
            println!("removed {}", path.display());
        }
        systemctl(&["daemon-reload"]).ok();
        println!("brrmmmm daemon uninstalled");
        Ok(())
    }

    fn systemctl(args: &[&str]) -> Result<()> {
        let status = std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .status()?;
        if !status.success() {
            anyhow::bail!("systemctl --user {} failed", args.join(" "));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use anyhow::Result;

    const LABEL: &str = "io.brrmmmm.daemon";
    const PLIST_NAME: &str = "io.brrmmmm.daemon.plist";

    fn plist_path() -> Result<std::path::PathBuf> {
        Ok(dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("no home directory"))?
            .join("Library/LaunchAgents")
            .join(PLIST_NAME))
    }

    fn uid() -> Result<String> {
        let out = std::process::Command::new("id").arg("-u").output()?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn launchctl(args: &[&str]) -> Result<()> {
        let status = std::process::Command::new("launchctl")
            .args(args)
            .status()?;
        if !status.success() {
            anyhow::bail!("launchctl {} failed", args.join(" "));
        }
        Ok(())
    }

    pub(super) fn install() -> Result<()> {
        let exe = std::env::current_exe()?;
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        let agents_dir = home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents_dir)?;

        let log_path = home.join(".brrmmmm/daemon.log");
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>{label}</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>{exe}</string>\n\
             \t\t<string>daemon</string>\n\
             \t\t<string>run</string>\n\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>KeepAlive</key>\n\
             \t<true/>\n\
             \t<key>StandardErrorPath</key>\n\
             \t<string>{log}</string>\n\
             </dict>\n\
             </plist>\n",
            label = LABEL,
            exe = exe.display(),
            log = log_path.display()
        );

        let plist_path = plist_path()?;
        std::fs::write(&plist_path, plist)?;
        let uid = uid()?;
        launchctl(&[
            "bootstrap",
            &format!("gui/{uid}"),
            &plist_path.to_string_lossy(),
        ])?;
        println!("installed {LABEL}");
        Ok(())
    }

    pub(super) fn start() -> Result<()> {
        let uid = uid()?;
        launchctl(&["kickstart", "-k", &format!("gui/{uid}/{LABEL}")])?;
        println!("brrmmmm daemon started");
        Ok(())
    }

    pub(super) fn stop() -> Result<()> {
        let uid = uid()?;
        launchctl(&["kill", "TERM", &format!("gui/{uid}/{LABEL}")])?;
        println!("brrmmmm daemon stopped");
        Ok(())
    }

    pub(super) fn restart() -> Result<()> {
        stop().ok();
        start()
    }

    pub(super) fn status() -> Result<()> {
        let uid = uid()?;
        let _ = std::process::Command::new("launchctl")
            .args(["print", &format!("gui/{uid}/{LABEL}")])
            .status();
        Ok(())
    }

    pub(super) fn uninstall() -> Result<()> {
        let uid = uid()?;
        let plist = plist_path()?;
        launchctl(&["bootout", &format!("gui/{uid}"), &plist.to_string_lossy()]).ok();
        if plist.exists() {
            std::fs::remove_file(&plist)?;
            println!("removed {}", plist.display());
        }
        println!("brrmmmm daemon uninstalled");
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use anyhow::Result;

    fn unsupported() -> Result<()> {
        anyhow::bail!("daemon service management is not supported on this OS")
    }

    pub(super) fn install() -> Result<()> {
        unsupported()
    }
    pub(super) fn start() -> Result<()> {
        unsupported()
    }
    pub(super) fn stop() -> Result<()> {
        unsupported()
    }
    pub(super) fn restart() -> Result<()> {
        unsupported()
    }
    pub(super) fn status() -> Result<()> {
        unsupported()
    }
    pub(super) fn uninstall() -> Result<()> {
        unsupported()
    }
}
