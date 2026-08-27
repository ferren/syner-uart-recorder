use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};

use crate::proto::{ChannelMode, Deframer, lost_between};
use crate::settings::Settings;

/// Live counters published by the capture thread for the UI to poll.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub bytes: u64,
    pub packets: u64,
    /// Packets presumed lost, inferred from gaps in the 2-bit `idx` counter.
    pub lost: u64,
    pub resync_bytes: u64,
    pub frames: u64,
    pub sample_rate: u32,
    /// Sample width reported in the packet headers; 16 until the first packet lands.
    pub bits: u8,
    /// Absolute peak per channel over the most recent window, normalised to 0.0..=1.0.
    pub peak: [f32; 2],
    pub error: Option<String>,
    pub stopped: bool,
}

impl Status {
    pub fn seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames as f64 / self.sample_rate as f64
        }
    }
}

/// Owns a running capture thread; dropping or calling [`Session::stop`] ends it.
pub struct Session {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    path: PathBuf,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Session {
    /// Opens `settings.port` and starts streaming into a new WAV file under
    /// [`crate::settings::recordings_dir`].
    pub fn start(settings: &Settings) -> Result<Self> {
        let port_name = settings
            .port
            .clone()
            .context("尚未选择串口")?;

        let dir = crate::settings::recordings_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("无法创建录音目录 {}", dir.display()))?;
        let path = dir.join(format!("{}.wav", timestamp()));

        // Fail fast on the main thread so the UI can show a real error instead of
        // silently spawning a thread that dies immediately.
        let mut port = serialport::new(&port_name, settings.baud)
            .timeout(Duration::from_millis(100))
            .open()
            .with_context(|| format!("无法打开串口 {port_name}"))?;

        // The firmware drops every packet unless the CDC line state reads 0x03
        // (see `usb_cdc_tx_request` in bluetooth/subsys/usb/if_usb.c), and
        // serialport-rs clears both lines on Windows. Assert them explicitly.
        port.write_data_terminal_ready(true)
            .context("无法置位 DTR (固件要求 DTR+RTS 均有效才会发送数据)")?;
        port.write_request_to_send(true)
            .context("无法置位 RTS (固件要求 DTR+RTS 均有效才会发送数据)")?;

        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status::default()));

        let handle = std::thread::Builder::new()
            .name("omi-capture".into())
            .spawn({
                let stop = stop.clone();
                let status = status.clone();
                let path = path.clone();
                let samples_per_packet = settings.samples_per_packet;
                let mode = settings.channel_mode;
                let rate_override = settings.sample_rate_override;
                move || {
                    let result = capture(
                        port,
                        &path,
                        samples_per_packet,
                        mode,
                        rate_override,
                        &stop,
                        &status,
                    );
                    let mut status = status.lock().unwrap();
                    status.stopped = true;
                    if let Err(err) = result {
                        status.error = Some(format!("{err:#}"));
                    }
                }
            })
            .context("无法启动采集线程")?;

        Ok(Self {
            stop,
            status,
            path,
            handle: Some(handle),
        })
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Signals the thread and waits for the WAV file to be finalised.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

fn capture(
    mut port: Box<dyn serialport::SerialPort>,
    path: &Path,
    samples_per_packet: usize,
    mode: ChannelMode,
    rate_override: Option<u32>,
    stop: &AtomicBool,
    status: &Mutex<Status>,
) -> Result<()> {
    // Whatever the OS buffered before we opened the port is mid-packet garbage.
    let _ = port.clear(serialport::ClearBuffer::All);

    let mut deframer = Deframer::new(samples_per_packet, mode);
    let mut read_buf = vec![0u8; 16 * 1024];
    let mut writer: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>> = None;
    let mut last_idx: Option<u8> = None;
    let channels = mode.channels();
    let mut peak = [0i32; 2];
    let mut peak_countdown = 0u32;

    while !stop.load(Ordering::Relaxed) {
        match port.read(&mut read_buf) {
            Ok(0) => {}
            Ok(n) => {
                deframer.extend(&read_buf[..n]);
                let mut status = status.lock().unwrap();
                status.bytes += n as u64;
            }
            // A read timeout just means the firmware had nothing to send.
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => return Err(anyhow::Error::new(e).context("串口读取失败")),
        }

        while let Some(packet) = deframer.next_packet() {
            let writer = match writer.as_mut() {
                Some(w) => w,
                None => {
                    let rate = rate_override.unwrap_or(packet.sample_rate);
                    let spec = hound::WavSpec {
                        channels,
                        sample_rate: rate,
                        bits_per_sample: 16,
                        sample_format: hound::SampleFormat::Int,
                    };
                    let w = hound::WavWriter::create(path, spec)
                        .with_context(|| format!("无法创建 {}", path.display()))?;
                    status.lock().unwrap().sample_rate = rate;
                    writer.insert(w)
                }
            };

            for (i, sample) in packet.samples.iter().enumerate() {
                writer.write_sample(*sample).context("WAV 写入失败")?;
                let channel = i % channels as usize;
                peak[channel] = peak[channel].max((*sample as i32).abs());
            }

            let lost = last_idx.map(|prev| lost_between(prev, packet.idx)).unwrap_or(0);
            last_idx = Some(packet.idx);

            let mut status = status.lock().unwrap();
            status.packets += 1;
            status.bits = packet.bits;
            status.lost += lost as u64;
            status.resync_bytes = deframer.resync_bytes;
            status.frames += (packet.samples.len() / channels as usize) as u64;

            // Decay the meter roughly every 10 packets so it tracks the signal
            // instead of latching onto an all-time maximum.
            peak_countdown += 1;
            if peak_countdown >= 10 {
                peak_countdown = 0;
                status.peak = [
                    peak[0] as f32 / i16::MAX as f32,
                    peak[1] as f32 / i16::MAX as f32,
                ];
                peak = [0, 0];
            }
        }
    }

    if let Some(writer) = writer {
        writer.finalize().context("WAV 收尾失败")?;
    } else {
        // Nothing arrived: don't leave a zero-byte file behind.
        let _ = std::fs::remove_file(path);
        anyhow::bail!("未收到任何有效数据包，请确认固件已开启 WAV_RECORD_EN 且串口/包长设置正确");
    }

    Ok(())
}

fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();

    // Minimal civil-time conversion (UTC) to avoid pulling in a date crate.
    let days = secs / 86_400;
    let tod = secs % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_derives_from_frames_and_rate() {
        let status = Status {
            frames: 32_000,
            sample_rate: 16_000,
            ..Default::default()
        };
        assert_eq!(status.seconds(), 2.0);
    }

    #[test]
    fn seconds_is_zero_before_the_rate_is_known() {
        assert_eq!(Status::default().seconds(), 0.0);
    }

    #[test]
    fn epoch_maps_to_1970() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn handles_leap_years() {
        // 2024-02-29 is 19782 days after the epoch.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }
}
