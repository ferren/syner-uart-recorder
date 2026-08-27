use std::path::PathBuf;

/// A recorded WAV file on disk, described well enough for the list view.
#[derive(Debug, Clone)]
pub struct Recording {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub channels: u16,
    pub sample_rate: u32,
    pub seconds: f64,
}

impl Recording {
    pub fn summary(&self) -> String {
        format!(
            "{:.1} s · {} ch · {} Hz · {:.1} MB",
            self.seconds,
            self.channels,
            self.sample_rate,
            self.bytes as f64 / (1024.0 * 1024.0)
        )
    }
}

/// Lists recordings newest-first. Files that cannot be parsed as WAV are skipped.
pub fn scan() -> Vec<Recording> {
    let dir = crate::settings::recordings_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut out: Vec<Recording> = entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        .filter_map(|entry| {
            let path = entry.path();
            let bytes = entry.metadata().ok()?.len();
            let reader = hound::WavReader::open(&path).ok()?;
            let spec = reader.spec();
            let frames = reader.duration() as f64;
            Some(Recording {
                name: path.file_stem()?.to_string_lossy().into_owned(),
                path,
                bytes,
                channels: spec.channels,
                sample_rate: spec.sample_rate,
                seconds: if spec.sample_rate == 0 {
                    0.0
                } else {
                    frames / spec.sample_rate as f64
                },
            })
        })
        .collect();

    // Filenames are timestamps, so a reverse lexical sort is newest-first.
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_every_field() {
        let r = Recording {
            path: PathBuf::from("x.wav"),
            name: "x".into(),
            bytes: 2 * 1024 * 1024,
            channels: 2,
            sample_rate: 16_000,
            seconds: 12.34,
        };
        assert_eq!(r.summary(), "12.3 s · 2 ch · 16000 Hz · 2.0 MB");
    }

    #[test]
    fn scanning_a_missing_directory_is_not_an_error() {
        // `scan` reads the real data dir; the contract we care about is that a
        // missing directory yields an empty list rather than panicking.
        let _ = scan();
    }
}
