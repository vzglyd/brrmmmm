pub fn find_tui_script(tui_path: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(val) = tui_path {
        let p = std::path::PathBuf::from(val);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // Packaged install: tui/ lives next to the binary
        let p = dir.join("tui/dist/index.js");
        if p.exists() {
            return Some(p);
        }
        // Dev layout: target/release/brrmmmm → ../../tui/dist/index.js
        let p = dir.join("../../tui/dist/index.js");
        if p.exists() {
            return Some(p);
        }
    }
    // CWD: user runs from within the cloned repo after cargo install --path .
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("tui/dist/index.js");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

pub fn launch_tui(args: &[String], config: &brrmmmm::config::Config) -> ! {
    let Some(tui) = find_tui_script(config.tui_path.as_deref()) else {
        eprintln!(
            "[brrmmmm] TUI not found. Build it with: npm --prefix tui run build\n\
             [brrmmmm] Or set BRRMMMM_TUI=/path/to/tui/dist/index.js"
        );
        std::process::exit(1);
    };

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new("node")
            .arg(&tui)
            .args(args)
            .exec();
        eprintln!("[brrmmmm] failed to exec node: {err}");
        std::process::exit(1);
    }

    #[cfg(not(unix))]
    {
        let status = std::process::Command::new("node")
            .arg(&tui)
            .args(args)
            .status()
            .unwrap_or_else(|e| {
                eprintln!("[brrmmmm] failed to launch node: {e}");
                std::process::exit(1);
            });
        std::process::exit(status.code().unwrap_or(1));
    }
}
