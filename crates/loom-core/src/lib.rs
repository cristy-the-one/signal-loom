//! Index and decode CAN / telemetry logs for Signal Loom.
//!
//! The index stores checkpoint offsets, not every sample. Callers ask for a
//! time window and a point budget; the UI never receives the whole recording.

mod analyze;
mod blf;
mod dbc;
mod decode;
mod dto;
mod error;
mod index;
mod jobs;
mod map;
mod project;
mod scan;
mod session;
mod socketcan;

#[cfg(test)]
mod tests;

pub use dto::{
    deserialize_us, BusLoad, EventDto, FrameDto, IndexStatus, MapMatch, OpenedProject, PointDto,
    ProjectOpen, Query, SeriesDto, SignalDto, StepDir, Summary, ValueDto, WindowStats,
};
pub use error::{Error, Result};
pub use index::{Export, IndexControl};
pub use jobs::Engine;
pub use map::SignalMap;
pub use project::{write_as, Bookmark, ProjectFile, ViewState, PROJECT_FORMAT};
pub use project::{MathChannel, Note, ThresholdTrigger, TriggerOp};
pub use session::{ProjectView, Session};
