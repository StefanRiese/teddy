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
    /// Where each track starts
    track_starts: Vec<Duration>,
}

impl TonieSource {
    pub fn open(path: &Path) -> Result<TonieSource, String> {
        let file = TonieFile::open(path, true).map_err(|e| e.to_string())?;
        /* a track starts after the last page of the one before */
        let track_starts = file
            .header
            .audio_chapters
            .iter()
            .map(|&page| match page {
                0 => 0,
                _ => file.granule_by_page(page - 1).unwrap_or(0),
            })
            .map(|granule| Duration::from_secs_f64(granule as f64 / 48000.0))
            .collect();
        let decoder = AudioDecoder::new(Arc::new(file.audio)).map_err(|e| e.to_string())?;
        Ok(TonieSource {
            decoder,
            buf: Vec::new(),
            pos: 0,
            track_starts,
        })
    }

    pub fn track_starts(&self) -> &[Duration] {
        &self.track_starts
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
    track_starts: Vec<Duration>,
}

/// Restarting the current track instead of going to the previous one after this much of it
const RESTART_AFTER: Duration = Duration::from_secs(3);

/// The track playing at `pos` (index into `starts`)
fn track_at(starts: &[Duration], pos: Duration) -> usize {
    starts.iter().rposition(|&s| s <= pos).unwrap_or(0)
}

/// Where "previous track" jumps: the start of the current track, or of the one before when the
/// current one has only just begun.
fn previous_start(starts: &[Duration], pos: Duration) -> Duration {
    let current = track_at(starts, pos);
    let start = starts.get(current).copied().unwrap_or_default();
    if pos.saturating_sub(start) > RESTART_AFTER || current == 0 {
        start
    } else {
        starts[current - 1]
    }
}

impl Player {
    pub fn new() -> Result<Player, String> {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        /* stopping on exit is intended, no message about it */
        sink.log_on_drop(false);
        Ok(Player {
            sink,
            player: None,
            duration: Duration::ZERO,
            track_starts: Vec::new(),
        })
    }

    pub fn play(&mut self, source: TonieSource) {
        self.stop();
        self.duration = source.duration();
        self.track_starts = source.track_starts().to_vec();
        let player = rodio::Player::connect_new(self.sink.mixer());
        player.append(source);
        self.player = Some(player);
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.player.take() {
            p.stop();
        }
    }

    /// Pauses at the current position; [`Player::resume`] continues from there.
    pub fn pause(&self) {
        if let Some(p) = &self.player {
            p.pause();
        }
    }

    pub fn resume(&self) {
        if let Some(p) = &self.player {
            p.play();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.player.as_ref().is_some_and(|p| p.is_paused())
    }

    /// Whether a Tonie is playing or paused
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

    pub fn track_count(&self) -> usize {
        self.track_starts.len()
    }

    /// The track playing now, counted from 0
    pub fn current_track(&self) -> usize {
        track_at(&self.track_starts, self.position())
    }

    pub fn play_track(&self, track: usize) {
        if let Some(&start) = self.track_starts.get(track) {
            self.seek(start);
        }
    }

    pub fn next_track(&self) {
        self.play_track(self.current_track() + 1);
    }

    pub fn previous_track(&self) {
        if !self.track_starts.is_empty() {
            self.seek(previous_start(&self.track_starts, self.position()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_navigation() {
        let s = |secs: u64| Duration::from_secs(secs);
        let starts = [s(0), s(60), s(120)];
        assert_eq!(track_at(&starts, s(0)), 0);
        assert_eq!(track_at(&starts, s(61)), 1);
        assert_eq!(track_at(&starts, s(500)), 2);
        /* just begun: the track before; later: the start of the current one */
        assert_eq!(previous_start(&starts, s(61)), s(0));
        assert_eq!(previous_start(&starts, s(90)), s(60));
        assert_eq!(previous_start(&starts, s(1)), s(0));

        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/reference/album_cbr96/0A0B0C01/500304E0");
        let source = TonieSource::open(&path).unwrap();
        let starts = source.track_starts();
        /* where the box starts a chapter: the start of its first page. teddy -m info, like Teddy.exe,
         * counts to the end of that page instead (4.72 s, 10.50 s), one page of 340 ms later */
        assert_eq!(starts.len(), 3);
        assert_eq!(starts[0], Duration::ZERO);
        assert!((starts[1].as_secs_f64() - 4.38).abs() < 0.01, "{starts:?}");
        assert!((starts[2].as_secs_f64() - 10.16).abs() < 0.01, "{starts:?}");
    }

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
