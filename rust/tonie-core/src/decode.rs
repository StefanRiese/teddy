//! Decoding the audio of a Tonie file to PCM, with seeking.

use std::sync::Arc;

use crate::ogg::Page;
use crate::writer::{CHANNELS, SAMPLE_RATE};
use crate::Error;

/// One Ogg page of the audio stream
struct PageIndex {
    offset: usize,
    /// Granule position at the end of the page
    granule: u64,
}

/// Decodes the Opus stream of a Tonie file to interleaved 48 kHz stereo `f32` samples.
pub struct AudioDecoder {
    audio: Arc<Vec<u8>>,
    pages: Vec<PageIndex>,
    decoder: opus::Decoder,
    next_page: usize,
    /// Decoded samples not yet handed out
    pending: Vec<f32>,
    pending_pos: usize,
    /// Frames to drop after a seek, to land exactly on the requested position
    skip_frames: u64,
    /// Position in frames of the next sample handed out
    position: u64,
}

impl AudioDecoder {
    /// `audio` is the audio part of a Tonie file (see [`crate::TonieFile::audio`]).
    pub fn new(audio: Arc<Vec<u8>>) -> Result<AudioDecoder, Error> {
        let mut pages = Vec::new();
        let mut offset = 0;
        while let Some((page, size)) = Page::parse(&audio, offset) {
            if !page.is_opus_header() {
                pages.push(PageIndex {
                    offset,
                    granule: page.granule_position,
                });
            }
            offset += size;
        }
        let decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Stereo).map_err(|e| Error::Decoder(e.to_string()))?;
        Ok(AudioDecoder {
            audio,
            pages,
            decoder,
            next_page: 0,
            pending: Vec::new(),
            pending_pos: 0,
            skip_frames: 0,
            position: 0,
        })
    }

    /// Length in frames (samples per channel)
    pub fn total_frames(&self) -> u64 {
        self.pages.last().map(|p| p.granule).unwrap_or(0)
    }

    /// Current position in frames
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Granule positions where each chapter starts, from the page numbers in the header.
    pub fn chapter_frames(&self, chapters: &[u32]) -> Vec<u64> {
        /* page n of the stream is entry n - 2 here, the two header pages are not indexed */
        chapters
            .iter()
            .map(|&c| match (c as usize).checked_sub(3) {
                Some(i) => self.pages.get(i).map(|p| p.granule).unwrap_or_else(|| self.total_frames()),
                None => 0,
            })
            .collect()
    }

    /// Continues decoding at `frame`.
    pub fn seek(&mut self, frame: u64) {
        let frame = frame.min(self.total_frames());
        /* first page that ends after the target; decoding starts there */
        let index = self.pages.partition_point(|p| p.granule <= frame);
        let start = if index == 0 { 0 } else { self.pages[index - 1].granule };
        self.next_page = index;
        self.pending.clear();
        self.pending_pos = 0;
        self.skip_frames = frame - start;
        self.position = frame;
        self.decoder.reset_state().ok();
    }

    /// Returns the next decoded samples, or `None` at the end of the stream.
    pub fn next_chunk(&mut self) -> Option<&[f32]> {
        loop {
            if self.pending_pos < self.pending.len() {
                let chunk = &self.pending[self.pending_pos..];
                self.pending_pos = self.pending.len();
                self.position += (chunk.len() / CHANNELS) as u64;
                return Some(chunk);
            }
            if !self.decode_page() {
                return None;
            }
        }
    }

    /// Decodes the next page into `pending`. Returns false at the end of the stream.
    fn decode_page(&mut self) -> bool {
        let Some(index) = self.pages.get(self.next_page) else { return false };
        self.next_page += 1;
        let Some((page, _)) = Page::parse(&self.audio, index.offset) else {
            return false;
        };

        self.pending.clear();
        self.pending_pos = 0;
        let mut pcm = vec![0f32; 5760 * CHANNELS];
        for packet in &page.segments {
            /* skip damaged packets instead of stopping playback */
            if let Ok(frames) = self.decoder.decode_float(packet, &mut pcm, false) {
                self.pending.extend_from_slice(&pcm[..frames * CHANNELS]);
            }
        }

        let skip = (self.skip_frames as usize * CHANNELS).min(self.pending.len());
        self.skip_frames -= (skip / CHANNELS) as u64;
        self.pending_pos = skip;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::{opus_encoder, OpusTags, TonieOggWriter, FRAME_SAMPLES};

    fn stream(frames: usize) -> Arc<Vec<u8>> {
        let mut w = TonieOggWriter::new(opus_encoder(96000, false).unwrap(), Vec::new(), &OpusTags::teddy(), 1).unwrap();
        let tone: Vec<i16> = (0..frames * FRAME_SAMPLES * CHANNELS)
            .map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16)
            .collect();
        w.write_samples(&tone).unwrap();
        Arc::new(w.finish().unwrap().0)
    }

    #[test]
    fn decodes_everything_and_seeks() {
        let audio = stream(200);
        let mut d = AudioDecoder::new(audio).unwrap();
        let total = d.total_frames();
        assert!(total >= 200 * FRAME_SAMPLES as u64);

        let mut decoded = 0u64;
        while let Some(chunk) = d.next_chunk() {
            decoded += (chunk.len() / CHANNELS) as u64;
        }
        assert_eq!(decoded, total);
        assert_eq!(d.position(), total);

        d.seek(total / 2 + 123);
        assert_eq!(d.position(), total / 2 + 123);
        let mut rest = 0u64;
        while let Some(chunk) = d.next_chunk() {
            rest += (chunk.len() / CHANNELS) as u64;
        }
        assert_eq!(rest, total - total / 2 - 123);
    }
}
