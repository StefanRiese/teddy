//! Decoding source audio files to 48 kHz stereo.

use std::cell::Cell;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::writer::{CHANNELS, FRAME_SAMPLES, SAMPLE_RATE};
use crate::Error;

/// File extensions accepted as encoder input
pub const SUPPORTED_EXTENSIONS: &[&str] = &["mp3", "ogg", "opus", "flac", "wav", "m4a", "m4b"];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Decodes `path` and hands interleaved 48 kHz stereo samples to `sink`.
/// `progress` receives the fraction of the file decoded so far.
pub fn decode_file(path: &Path, sink: &mut dyn FnMut(&[f32]) -> Result<(), Error>, progress: &mut dyn FnMut(f64)) -> Result<(), Error> {
    if is_ogg_opus(path)? {
        decode_ogg_opus(path, sink, progress)
    } else {
        decode_symphonia(path, sink, progress)
    }
}

fn is_ogg_opus(path: &Path) -> Result<bool, Error> {
    let mut head = [0u8; 36];
    let mut file = File::open(path)?;
    let n = file.read(&mut head)?;
    /* first page holds the OpusHead packet right after the page header with one lacing value */
    Ok(n >= 36 && &head[0..4] == b"OggS" && &head[28..36] == b"OpusHead")
}

/// Wraps a reader and counts how many bytes were read, for progress reports.
struct CountingReader<R> {
    inner: R,
    count: Rc<Cell<u64>>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.set(self.count.get() + n as u64);
        Ok(n)
    }
}

impl<R: Seek> Seek for CountingReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let p = self.inner.seek(pos)?;
        self.count.set(p);
        Ok(p)
    }
}

fn decode_ogg_opus(path: &Path, sink: &mut dyn FnMut(&[f32]) -> Result<(), Error>, progress: &mut dyn FnMut(f64)) -> Result<(), Error> {
    let file = File::open(path)?;
    let total = file.metadata()?.len().max(1);
    let count = Rc::new(Cell::new(0));
    let mut reader = ogg::reading::PacketReader::new(CountingReader {
        inner: BufReader::new(file),
        count: count.clone(),
    });

    let invalid = |msg: &str| Error::InvalidFile(format!("{}: {msg}", path.display()));
    let mut next_packet = || reader.read_packet().map_err(|e| invalid(&e.to_string()));

    let head = next_packet()?.ok_or_else(|| invalid("missing OpusHead"))?;
    if head.data.len() < 19 || &head.data[0..8] != b"OpusHead" {
        return Err(invalid("missing OpusHead"));
    }
    let mut pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as usize;
    /* OpusTags */
    next_packet()?.ok_or_else(|| invalid("missing OpusTags"))?;

    let mut decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Stereo).map_err(|e| Error::Decoder(e.to_string()))?;
    /* the longest Opus packet holds 120 ms */
    let mut pcm = vec![0f32; 5760 * CHANNELS];
    while let Some(packet) = next_packet()? {
        let frames = decoder.decode_float(&packet.data, &mut pcm, false).map_err(|e| Error::Decoder(e.to_string()))?;
        let skip = pre_skip.min(frames);
        pre_skip -= skip;
        sink(&pcm[skip * CHANNELS..frames * CHANNELS])?;
        progress(count.get() as f64 / total as f64);
    }
    Ok(())
}

/// The Opus packets of an Ogg Opus file that can go into a Tonie file as they are, without
/// re-encoding: stereo, and every packet 20 ms like the frames the Toniebox uses. `None` for other
/// files, they are encoded. Padding is removed; the Tonie writer pads the packets to fill its pages.
pub fn tonie_ready_opus_packets(path: &Path) -> Result<Option<Vec<Vec<u8>>>, Error> {
    if !is_ogg_opus(path)? {
        return Ok(None);
    }
    let mut reader = ogg::reading::PacketReader::new(BufReader::new(File::open(path)?));
    let invalid = |msg: &str| Error::InvalidFile(format!("{}: {msg}", path.display()));
    let mut next_packet = || reader.read_packet().map_err(|e| invalid(&e.to_string()));

    let head = next_packet()?.ok_or_else(|| invalid("missing OpusHead"))?;
    /* channels, and the channel mapping family 0 (mono or stereo, no multistream) */
    if head.data.len() < 19 || head.data[9] != CHANNELS as u8 || head.data[18] != 0 {
        return Ok(None);
    }
    next_packet()?.ok_or_else(|| invalid("missing OpusTags"))?;

    let mut packets = Vec::new();
    while let Some(packet) = next_packet()? {
        let mut data = packet.data;
        if opus::packet::get_nb_samples(&data, SAMPLE_RATE).ok() != Some(FRAME_SAMPLES) {
            return Ok(None);
        }
        if let Ok(len) = opus::packet::unpad(&mut data) {
            data.truncate(len);
        }
        packets.push(data);
    }
    Ok((!packets.is_empty()).then_some(packets))
}

fn decode_symphonia(path: &Path, sink: &mut dyn FnMut(&[f32]) -> Result<(), Error>, progress: &mut dyn FnMut(f64)) -> Result<(), Error> {
    let sym = |e: SymError| Error::Decoder(format!("{}: {e}", path.display()));

    let mss = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(sym)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error::Decoder(format!("{}: no audio track", path.display())))?;
    let track_id = track.id;
    let total_frames = track.num_frames;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| Error::Decoder(format!("{}: no audio track", path.display())))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(sym)?;

    let mut resampler: Option<(u32, StereoResampler)> = None;
    let mut decoded = Vec::new();
    let mut stereo = Vec::new();
    let mut frames_done = 0u64;

    while let Some(packet) = format.next_packet().map_err(sym)? {
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            /* skip corrupt frames like other players do */
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(sym(e)),
        };

        let rate = buf.spec().rate();
        let channels = buf.spec().channels().count().max(1);
        decoded.resize(buf.samples_interleaved(), 0f32);
        buf.copy_to_slice_interleaved(&mut decoded);
        frames_done += buf.frames() as u64;

        to_stereo(&decoded, channels, &mut stereo);
        let (r, rs) = resampler.get_or_insert_with(|| (rate, StereoResampler::new(rate)));
        if *r != rate {
            return Err(Error::Decoder(format!("{}: sample rate changes within the file", path.display())));
        }
        rs.push(&stereo, sink)?;

        if let Some(total) = total_frames {
            progress(frames_done as f64 / total.max(1) as f64);
        }
    }

    if let Some((_, rs)) = resampler.as_mut() {
        rs.finish(sink)?;
    }
    Ok(())
}

/// Maps any channel layout to stereo: mono is duplicated, extra channels are dropped.
fn to_stereo(input: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    for frame in input.chunks_exact(channels) {
        let l = frame[0];
        let r = if channels > 1 { frame[1] } else { l };
        out.push(l);
        out.push(r);
    }
}

/// Streaming stereo resampler to 48 kHz. Passes the audio through if it already is 48 kHz.
struct StereoResampler {
    fft: Option<Fft<f32>>,
    pending: Vec<f32>,
    output: Vec<f32>,
    /// Output frames still to drop at the start, the resampler's delay
    skip: usize,
    frames_in: u64,
    frames_out: u64,
    rate_in: u32,
}

impl StereoResampler {
    const CHUNK: usize = 1024;

    fn new(rate_in: u32) -> StereoResampler {
        let fft = (rate_in != SAMPLE_RATE)
            .then(|| Fft::<f32>::new(rate_in as usize, SAMPLE_RATE as usize, Self::CHUNK, CHANNELS, FixedSync::Input).expect("valid resampler parameters"));
        let (skip, out_max) = fft.as_ref().map(|f| (f.output_delay(), f.output_frames_max())).unwrap_or((0, 0));
        StereoResampler {
            fft,
            pending: Vec::new(),
            output: vec![0f32; out_max * CHANNELS],
            skip,
            frames_in: 0,
            frames_out: 0,
            rate_in,
        }
    }

    fn push(&mut self, data: &[f32], sink: &mut dyn FnMut(&[f32]) -> Result<(), Error>) -> Result<(), Error> {
        if self.fft.is_none() {
            return sink(data);
        }
        self.frames_in += (data.len() / CHANNELS) as u64;
        self.pending.extend_from_slice(data);
        while self.pending.len() >= Self::CHUNK * CHANNELS {
            self.process(Self::CHUNK, sink)?;
            self.pending.drain(..Self::CHUNK * CHANNELS);
        }
        Ok(())
    }

    fn finish(&mut self, sink: &mut dyn FnMut(&[f32]) -> Result<(), Error>) -> Result<(), Error> {
        if self.fft.is_none() {
            return Ok(());
        }
        let expected = self.frames_in * u64::from(SAMPLE_RATE) / u64::from(self.rate_in);
        /* flush the last partial chunk, then zeros until the delayed output is complete */
        let mut partial = self.pending.len() / CHANNELS;
        while self.frames_out < expected {
            self.pending.resize(Self::CHUNK * CHANNELS, 0.0);
            self.process(partial, sink)?;
            self.pending.clear();
            partial = 0;
        }
        Ok(())
    }

    fn process(&mut self, valid_frames: usize, sink: &mut dyn FnMut(&[f32]) -> Result<(), Error>) -> Result<(), Error> {
        let fft = self.fft.as_mut().unwrap();
        let input = InterleavedSlice::new(&self.pending[..Self::CHUNK * CHANNELS], CHANNELS, Self::CHUNK).unwrap();
        let out_frames = self.output.len() / CHANNELS;
        let mut output = InterleavedSlice::new_mut(&mut self.output, CHANNELS, out_frames).unwrap();
        let indexing = Indexing {
            partial_len: (valid_frames < Self::CHUNK).then_some(valid_frames),
            ..Default::default()
        };
        let (_, produced) = fft
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|e| Error::Decoder(format!("resampling failed: {e}")))?;

        let skip = self.skip.min(produced);
        self.skip -= skip;
        let expected = self.frames_in * u64::from(SAMPLE_RATE) / u64::from(self.rate_in);
        let usable = ((produced - skip) as u64).min(expected.saturating_sub(self.frames_out)) as usize;
        self.frames_out += usable as u64;
        if usable > 0 {
            sink(&self.output[skip * CHANNELS..(skip + usable) * CHANNELS])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/inputs").join(name)
    }

    /// Seconds of 48 kHz stereo audio decoded from the file
    fn decoded_seconds(name: &str) -> f64 {
        let mut samples = 0usize;
        decode_file(
            &input(name),
            &mut |s| {
                samples += s.len();
                Ok(())
            },
            &mut |_| {},
        )
        .unwrap();
        samples as f64 / 2.0 / 48_000.0
    }

    #[test]
    fn decodes_m4a_and_ogg_vorbis() {
        for (name, seconds) in [("aac_input.m4a", 2.0), ("vorbis_input.ogg", 2.0), ("opus_input.ogg", 4.0)] {
            assert!(is_supported(&input(name)), "{name}");
            let got = decoded_seconds(name);
            assert!((got - seconds).abs() < 0.1, "{name}: {got} s");
        }
    }
}
