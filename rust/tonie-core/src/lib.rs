//! Reading and writing the audio files of the Toniebox.
//!
//! A Tonie file is a 4 KiB protobuf header ([`header::FileHeader`]) followed by an Ogg Opus
//! stream (48 kHz stereo, 20 ms frames) whose pages are aligned to 4 KiB blocks.

pub mod audio_in;
pub mod encode;
pub mod file;
pub mod header;
pub mod info;
pub mod ogg;
pub mod tonies;
pub mod writer;

pub use encode::{build_file_list, encode, EncodeOptions, EncodeProgress};
pub use file::TonieFile;
pub use header::FileHeader;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid header: {0}")]
    InvalidHeader(String),
    #[error("invalid file: {0}")]
    InvalidFile(String),
    #[error("{0}")]
    NotFound(String),
    #[error("decoding failed: {0}")]
    Decoder(String),
    #[error("encoding failed: {0}")]
    Encoder(String),
    #[error("{0}")]
    Padding(String),
    #[error("{0}")]
    PrefixFailed(String),
}
