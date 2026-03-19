use crate::transport::SessionConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use velin_proto::{DEFAULT_AUDIO_PORT, DEFAULT_CONTROL_PORT};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub target_ip: String,
    pub bind_ip: String,
    pub output_device_name: String,
    pub control_port: u16,
    pub audio_port: u16,
    pub theme_mode: ThemeMode,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            target_ip: "127.0.0.1".to_string(),
            bind_ip: "0.0.0.0".to_string(),
            output_device_name: String::new(),
            control_port: DEFAULT_CONTROL_PORT,
            audio_port: DEFAULT_AUDIO_PORT,
            theme_mode: ThemeMode::Dark,
        }
    }
}

impl AppSettings {
    pub fn session_config(&self) -> SessionConfig {
        SessionConfig {
            target_ip: self.target_ip.clone(),
            bind_ip: self.bind_ip.clone(),
            output_device_name: self.output_device_name.clone(),
            control_port: self.control_port,
            audio_port: self.audio_port,
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new() -> Result<Self> {
        Ok(Self {
            path: settings_path()?,
        })
    }

    pub fn load_or_default(&self) -> Result<AppSettings> {
        if !self.path.exists() {
            return Ok(AppSettings::default());
        }

        let bytes = fs::read(&self.path)
            .with_context(|| format!("failed to read settings file {}", self.path.display()))?;
        serde_json::from_slice(&bytes).context("failed to parse settings file")
    }

    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create settings directory {}", parent.display())
            })?;
        }

        let bytes = serde_json::to_vec_pretty(settings).context("failed to serialize settings")?;
        fs::write(&self.path, bytes)
            .with_context(|| format!("failed to write settings file {}", self.path.display()))
    }
}
fn settings_path() -> Result<PathBuf> {
    if cfg!(target_os = "windows") {
        if let Ok(appdata) = env::var("APPDATA") {
            return Ok(PathBuf::from(appdata).join("velin").join("settings.json"));
        }
    }

    if let Ok(xdg_config_home) = env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg_config_home).join("velin").join("settings.json"));
    }

    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .context("could not determine home directory for settings")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("velin")
        .join("settings.json"))
}
