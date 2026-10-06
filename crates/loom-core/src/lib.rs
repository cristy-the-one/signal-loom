//! Index and decode CAN / telemetry logs for Signal Loom.
//!
//! The index stores checkpoint offsets, not every sample. Callers ask for a
//! time window and a point budget; the UI never receives the whole recording.

mod decode;
mod dto;
mod error;
mod index;
mod map;
mod project;
mod scan;
mod session;

#[cfg(test)]
mod tests;

pub use dto::{
    deserialize_us, EventDto, FrameDto, PointDto, ProjectOpen, Query, SeriesDto, SignalDto,
    StepDir, Summary, ValueDto,
};
pub use error::{Error, Result};
pub use project::{Bookmark, ProjectFile, ViewState, PROJECT_FORMAT};
pub use session::Session;
