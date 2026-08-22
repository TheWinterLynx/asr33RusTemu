//! Deterministic paper-tape reader and punch domain logic.

mod punch;
mod reader;
mod tape;

pub use punch::{PunchMode, PunchState, TapePunch};
pub use reader::{ReaderOptions, ReaderState, ReaderStep, SeekError, StopCause, TapeReader};
pub use tape::{PaperTape, TrailerPositions};
