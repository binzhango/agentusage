use anyhow::Result;
use std::{env, path::PathBuf};

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
        _ => "codex",
    };
    Ok(default_state_dir()?.join(format!("{name}.db")))
}
