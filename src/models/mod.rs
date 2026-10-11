//! Model catalogue, hardware probe, and the recommendation function.

mod catalogue;
mod hardware;

pub use catalogue::{bundled, parse, ram_budget, recommend, Catalogue, Kind, Model, Ranked};
pub use hardware::{classify_gpu, Accel, GpuKind, Hardware};
