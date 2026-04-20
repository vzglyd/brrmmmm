use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub tui_path: Option<String>,
    pub ai_model: String,
    pub browser_headless: bool,
    pub attestation_disabled: bool,
    pub identity_dir: PathBuf,
    pub state_dir: PathBuf,
    pub anthropic_api_key: Option<String>,
}

impl Config {
    pub fn load() -> Self {
        let attestation_disabled = std::env::var("BRRMMMM_ATTESTATION")
            .map(|value| {
                let value = value.trim();
                value == "0"
                    || value.eq_ignore_ascii_case("off")
                    || value.eq_ignore_ascii_case("false")
                    || value.eq_ignore_ascii_case("no")
                    || value.eq_ignore_ascii_case("legacy")
            })
            .unwrap_or(false);

        let browser_headless = std::env::var("BRRMMMM_BROWSER_HEADLESS")
            .map(|value| !matches!(value.trim().to_lowercase().as_str(), "false" | "0" | "no" | "off"))
            .unwrap_or(true);

        let ai_model = std::env::var("BRRMMMM_AI_MODEL")
            .unwrap_or_else(|_| "claude-3-5-sonnet-20241022".to_string());

        let tui_path = std::env::var("BRRMMMM_TUI").ok();

        let identity_dir = std::env::var_os("BRRMMMM_IDENTITY_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .map(|mut p| {
                        p.push("brrmmmm");
                        p.push("identity");
                        p
                    })
                    .unwrap_or_else(|| {
                        // Fallback to legacy behavior if data_dir fails
                        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
                        home.join(".local/share/brrmmmm/identity")
                    })
            });

        let state_dir = std::env::var_os("BRRMMMM_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .map(|mut p| {
                        p.push("brrmmmm");
                        p.push("state");
                        p
                    })
                    .unwrap_or_else(|| {
                        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
                        home.join(".local/share/brrmmmm/state")
                    })
            });

        let anthropic_api_key = std::env::var("ANTHROPIC_API_KEY").ok();

        Self {
            tui_path,
            ai_model,
            browser_headless,
            attestation_disabled,
            identity_dir,
            state_dir,
            anthropic_api_key,
        }
    }
}
