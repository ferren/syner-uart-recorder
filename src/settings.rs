use std::path::{Path, PathBuf};

use crate::proto::{ChannelMode, DEFAULT_SAMPLES_PER_PACKET};

/// Baud rates offered in the UI. The firmware's CDC endpoint ignores the rate, but
/// the host driver still wants a plausible value.
pub const BAUD_PRESETS: [u32; 5] = [115_200, 921_600, 1_000_000, 2_000_000, 3_000_000];

/// `WAV_SAMPLES_PER_PACKET` variants worth offering.
pub const SAMPLES_PRESETS: [usize; 3] = [160, 320, 640];

/// `None` means "trust the sample_rate field in the packet header".
pub const RATE_PRESETS: [Option<u32>; 3] = [None, Some(16_000), Some(8_000)];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub port: Option<String>,
    pub baud: u32,
    pub samples_per_packet: usize,
    pub channel_mode: ChannelMode,
    /// Overrides the header-reported rate when set.
    pub sample_rate_override: Option<u32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: None,
            baud: BAUD_PRESETS[0],
            samples_per_packet: DEFAULT_SAMPLES_PER_PACKET,
            channel_mode: ChannelMode::Both,
            sample_rate_override: None,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }

    pub fn cycle_baud(&mut self) {
        self.baud = cycle(&BAUD_PRESETS, &self.baud);
    }

    pub fn cycle_samples(&mut self) {
        self.samples_per_packet = cycle(&SAMPLES_PRESETS, &self.samples_per_packet);
    }

    pub fn cycle_rate_override(&mut self) {
        self.sample_rate_override = cycle(&RATE_PRESETS, &self.sample_rate_override);
    }

    pub fn rate_override_label(&self) -> String {
        match self.sample_rate_override {
            None => "自动 (读取包头)".to_string(),
            Some(rate) => format!("{rate} Hz (强制)"),
        }
    }
}

fn cycle<T: PartialEq + Clone>(options: &[T], current: &T) -> T {
    let at = options.iter().position(|o| o == current).unwrap_or(0);
    options[(at + 1) % options.len()].clone()
}

/// Root for settings and recordings: `%APPDATA%\syner-uart-recorder` on Windows,
/// `$HOME/.syner-uart-recorder` elsewhere, falling back to the working directory.
pub fn data_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Path::new(&appdata).join("syner-uart-recorder");
    }
    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home).join(".syner-uart-recorder");
    }
    PathBuf::from("syner-uart-recorder-data")
}

pub fn recordings_dir() -> PathBuf {
    data_dir().join("recordings")
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_through_presets_and_wraps() {
        let mut s = Settings::default();
        assert_eq!(s.baud, 115_200);
        s.cycle_baud();
        assert_eq!(s.baud, 921_600);
        for _ in 0..(BAUD_PRESETS.len() - 1) {
            s.cycle_baud();
        }
        assert_eq!(s.baud, BAUD_PRESETS[0], "wraps back around the preset list");
    }

    #[test]
    fn cycles_rate_override_including_auto() {
        let mut s = Settings::default();
        assert_eq!(s.sample_rate_override, None);
        s.cycle_rate_override();
        assert_eq!(s.sample_rate_override, Some(16_000));
        s.cycle_rate_override();
        assert_eq!(s.sample_rate_override, Some(8_000));
        s.cycle_rate_override();
        assert_eq!(s.sample_rate_override, None);
    }

    #[test]
    fn unknown_current_value_restarts_the_cycle() {
        let mut s = Settings {
            baud: 4_321,
            ..Default::default()
        };
        s.cycle_baud();
        assert_eq!(s.baud, BAUD_PRESETS[1]);
    }
}
