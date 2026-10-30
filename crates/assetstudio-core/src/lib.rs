//! Native application layer around the pinned Unity format backend.
#![forbid(unsafe_code)]

mod animation;
pub mod asset_map;
pub mod cab_map;
mod catalog;
pub mod class_structures;
mod export;
mod filter;
mod font_preview;
pub mod game_objects;
pub mod games;
mod model;
mod model_motion;
mod model_options;
mod model_preview;
mod output;
pub mod recovery;
pub mod schema;
mod session;
mod source_protection;
pub mod unity_cn_keys;

pub use catalog::{
    AssetKey, AssetRecord, ExportMode, ExportOptions, GroupMode, ImageFormat, LoadOptions,
    MapFormat, ModelFormat, OperationReport, Preview, SceneNode, class_id, class_name,
};
pub use filter::AssetFilter;
pub use session::Session;
pub use unity_rs_core::loader::{
    LoadProgress as ProgressUpdate, LoadProgressObserver as ProgressObserver,
};

pub use model_options::ModelExportOptions;
pub use unity_rs_core::fbx_ascii::{FbxExportSettings, FbxUvType};
