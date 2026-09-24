use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AutoReset {
    pub weekly_enabled: bool,
    pub weekly_percent: f64,
    pub skip_within_hours: f64,
    pub five_hour_enabled: bool,
    pub five_hour_minutes: f64,
}

impl Default for AutoReset {
    fn default() -> Self {
        Self { weekly_enabled: false, weekly_percent: 95.0, skip_within_hours: 24.0, five_hour_enabled: false, five_hour_minutes: 60.0 }
    }
}

impl AutoReset {
    fn clamped(self) -> Self {
        Self {
            weekly_percent: self.weekly_percent.clamp(1.0, 100.0).round(),
            skip_within_hours: self.skip_within_hours.clamp(0.0, 168.0).round(),
            five_hour_minutes: self.five_hour_minutes.clamp(0.0, 300.0).round(),
            ..self
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub codex_auto_reset: AutoReset,
}

pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> Self {
        let current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, current: Mutex::new(current) }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    pub fn save(&self, settings: Settings) -> Result<Settings, String> {
        let settings = Settings { codex_auto_reset: settings.codex_auto_reset.clamped() };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't save settings: {e}"))?;
        }
        let json = serde_json::to_string_pretty(&settings).expect("settings serialize");
        std::fs::write(&self.path, json).map_err(|e| format!("Couldn't save settings: {e}"))?;
        *self.current.lock().unwrap() = settings.clone();
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_clamped_values_and_reloads_them() {
        let path = std::env::temp_dir().join(format!("ai-usage-settings-{}.json", std::process::id()));
        let store = SettingsStore::load(path.clone());
        assert_eq!(store.get(), Settings::default());

        let wild = AutoReset { weekly_enabled: true, weekly_percent: 250.0, skip_within_hours: -3.0, five_hour_minutes: 59.6, ..AutoReset::default() };
        let saved = store.save(Settings { codex_auto_reset: wild }).unwrap();
        assert_eq!(
            saved.codex_auto_reset,
            AutoReset { weekly_enabled: true, weekly_percent: 100.0, skip_within_hours: 0.0, five_hour_enabled: false, five_hour_minutes: 60.0 }
        );
        assert_eq!(SettingsStore::load(path.clone()).get(), saved);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let settings: Settings = serde_json::from_str(r#"{"codexAutoReset": {"weeklyEnabled": true}}"#).unwrap();
        assert!(settings.codex_auto_reset.weekly_enabled);
        assert_eq!(settings.codex_auto_reset.weekly_percent, 95.0);
    }
}
