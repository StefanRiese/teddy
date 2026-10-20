//! Playing Tonie files.

use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rodio::source::SeekError;
use rodio::Source;
use tonie_core::decode::AudioDecoder;
use tonie_core::TonieFile;

/// The audio of a Tonie file as a rodio source.
pub struct TonieSource {
    decoder: AudioDecoder,
    buf: Vec<f32>,
    pos: usize,
}

impl TonieSource {
    pub fn open(path: &Path) -> Result<TonieSource, String> {
        let file = TonieFile::open(path, true).map_err(|e| e.to_string())?;
        let decoder = AudioDecoder::new(Arc::new(file.audio)).map_err(|e| e.to_string())?;
        Ok(TonieSource {
            decoder,
            buf: Vec::new(),
            pos: 0,
        })
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.decoder.total_frames() as f64 / 48000.0)
    }
}

impl Iterator for TonieSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buf.len() {
            let chunk = self.decoder.next_chunk()?;
            self.buf.clear();
            self.buf.extend_from_slice(chunk);
            self.pos = 0;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for TonieSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(2).unwrap()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(48000).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(self.duration())
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.decoder.seek((pos.as_secs_f64() * 48000.0) as u64);
        self.buf.clear();
        self.pos = 0;
        Ok(())
    }
}

/// Plays one Tonie at a time on the default output device.
pub struct Player {
    sink: rodio::MixerDeviceSink,
    player: Option<rodio::Player>,
    duration: Duration,
}

impl Player {
    pub fn new() -> Result<Player, String> {
        let sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        Ok(Player {
            sink,
            player: None,
            duration: Duration::ZERO,
        })
    }

    pub fn play(&mut self, source: TonieSource) {
        self.stop();
        self.duration = source.duration();
        let player = rodio::Player::connect_new(self.sink.mixer());
        player.append(source);
        self.player = Some(player);
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.player.take() {
            p.stop();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.player.as_ref().map(|p| !p.empty()).unwrap_or(false)
    }

    pub fn position(&self) -> Duration {
        self.player.as_ref().map(|p| p.get_pos()).unwrap_or_default()
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn seek(&self, pos: Duration) {
        if let Some(p) = &self.player {
            p.try_seek(pos).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_plays_and_seeks_without_an_output_device() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/reference/album_cbr96/0A0B0C01/500304E0");
        let mut source = TonieSource::open(&path).unwrap();
        let duration = source.duration();
        assert!((duration.as_secs_f64() - 13.24).abs() < 0.01, "{duration:?}");

        let total = source.by_ref().count();
        assert_eq!(total as f64, (duration.as_secs_f64() * 48000.0 * 2.0).round());

        source.try_seek(Duration::from_secs(10)).unwrap();
        let rest = source.count();
        assert!((rest as f64 / 96000.0 - (duration.as_secs_f64() - 10.0)).abs() < 0.001, "{rest}");
    }
}
