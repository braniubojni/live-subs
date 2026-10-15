//! Model catalogue, hardware probe, and the recommendation function.

mod catalogue;
mod download;
mod hardware;
mod hf;
mod validate;

pub use catalogue::{bundled, parse, ram_budget, recommend, Catalogue, Kind, Model, Ranked};
pub use download::{fetch, model_dir, remove_all_in, remove_in, url_file_name};
pub use hardware::{classify_gpu, Accel, GpuKind, Hardware};
pub use hf::{parse_models, HfHub, ModelSearch, RemoteFile};
pub use validate::validate_file;
