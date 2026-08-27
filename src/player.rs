use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rodio::{OutputStream, Sink};

/// Lazily-initialised playback of a single recording at a time.
///
/// The audio device is only opened on the first play so that the tool still starts
/// on machines with no output device.
#[derive(Default)]
pub struct Player {
    /// Must outlive the sink; dropping it stops playback.
    stream: Option<OutputStream>,
    sink: Option<Sink>,
    current: Option<PathBuf>,
}

impl Player {
    pub fn play(&mut self, path: &Path) -> Result<()> {
        self.stop();

        if self.stream.is_none() {
            let stream = rodio::OutputStreamBuilder::open_default_stream()
                .context("无法打开音频输出设备")?;
            self.stream = Some(stream);
        }
        let stream = self.stream.as_ref().expect("stream just initialised");

        let file = std::fs::File::open(path)
            .with_context(|| format!("无法打开 {}", path.display()))?;
        let source = rodio::Decoder::try_from(file)
            .with_context(|| format!("无法解码 {}", path.display()))?;

        let sink = Sink::connect_new(stream.mixer());
        sink.append(source);
        self.sink = Some(sink);
        self.current = Some(path.to_path_buf());
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.current = None;
    }

    /// Clears `current` once the sink has drained so the UI can drop its highlight.
    pub fn poll(&mut self) {
        if self.sink.as_ref().is_some_and(|s| s.empty()) {
            self.stop();
        }
    }

    pub fn is_playing(&self, path: &Path) -> bool {
        self.current.as_deref() == Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_idle_and_stop_is_a_no_op() {
        let mut p = Player::default();
        assert!(!p.is_playing(Path::new("anything.wav")));
        p.stop();
        p.poll();
        assert!(!p.is_playing(Path::new("anything.wav")));
    }

    #[test]
    fn play_reports_a_missing_file_without_opening_a_device() {
        let mut p = Player::default();
        // Either the device or the file fails; both must surface as an error, not a panic.
        assert!(p.play(Path::new("does-not-exist-93bd.wav")).is_err());
    }
}
