//! Integrations with operating-system and external I/O facilities.

#[path = "audio_margin.rs"]
pub mod audio;
#[path = "audio_rodio_crlf.rs"]
mod audio_rodio_crlf;
pub mod paper_tape;
pub mod transport;
