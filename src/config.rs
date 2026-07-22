use anyhow::Result;
use std::{env, fs, path::PathBuf, time::Duration};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub auto_sync: bool,
    pub refresh_interval: Duration,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            auto_sync: true,
            refresh_interval: Duration::from_secs(5 * 60),
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Ok(value) = env::var("AGENTUSAGE_CONFIG")
        && !value.trim().is_empty()
    {
        return Ok(PathBuf::from(value));
    }
    let home = env::var("HOME").or_else(|_| env::var("USERPROFILE"))?;
    #[cfg(target_os = "windows")]
    let base = env::var("APPDATA").unwrap_or_else(|_| format!("{home}\\AppData\\Roaming"));
    #[cfg(not(target_os = "windows"))]
    let base = env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{home}/.config"));
    Ok(PathBuf::from(base).join("agentusage").join("config.toml"))
}

pub fn load() -> Result<AppConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("read config {}: {error}", path.display()))?;
    parse(&text).map_err(|error| anyhow::anyhow!("parse config {}: {error}", path.display()))
}

fn parse(text: &str) -> Result<AppConfig, String> {
    let mut config = AppConfig::default();
    let mut section = String::new();
    for (line_number, raw_line) in text.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {} must contain key = value", line_number + 1));
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches('"');
        match (section.as_str(), key.as_str()) {
            ("sync", "auto_sync") | ("", "auto_sync") => {
                config.auto_sync = match value {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(format!(
                            "line {}: auto_sync must be true or false",
                            line_number + 1
                        ));
                    }
                };
            }
            ("sync", "refresh_seconds")
            | ("sync", "interval_seconds")
            | ("", "refresh_seconds")
            | ("", "sync_interval_seconds") => {
                let seconds = value.parse::<u64>().map_err(|_| {
                    format!(
                        "line {}: refresh interval must be an integer",
                        line_number + 1
                    )
                })?;
                if seconds == 0 {
                    return Err(format!(
                        "line {}: refresh interval must be greater than zero",
                        line_number + 1
                    ));
                }
                config.refresh_interval = Duration::from_secs(seconds);
            }
            _ => {}
        }
    }
    Ok(config)
}

pub fn default_state_dir() -> Result<PathBuf> {
    if let Ok(value) = env::var("XDG_STATE_HOME")
        && !value.trim().is_empty()
    {
        return Ok(PathBuf::from(value).join("agentusage"));
    }
    let home = env::var("HOME").or_else(|_| env::var("USERPROFILE"))?;
    #[cfg(target_os = "windows")]
    let base = env::var("LOCALAPPDATA").unwrap_or_else(|_| format!("{home}\\AppData\\Local"));
    #[cfg(not(target_os = "windows"))]
    let base = format!("{home}/.local/state");
    Ok(PathBuf::from(base).join("agentusage"))
}

pub fn default_db_path() -> Result<PathBuf> {
    Ok(default_state_dir()?.join("telemetry.db"))
}

pub fn agent_db_path(agent: &str) -> Result<PathBuf> {
    let name = match agent {
        "claude" | "claude_code" => "claude_code",
        "opencode" | "open_code" => "opencode",
        "copilot" => "copilot",
        "pi" => "pi",
        _ => "codex",
    };
    Ok(default_state_dir()?.join(format!("{name}.db")))
}
