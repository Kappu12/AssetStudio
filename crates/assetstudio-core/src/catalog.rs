use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AssetKey {
    pub file_index: usize,
    pub path_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRecord {
    pub key: AssetKey,
    pub class_id: i32,
    pub class_name: String,
    pub name: String,
    pub container: String,
    pub source: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct LoadOptions {
    pub game: String,
    pub unity_version: Option<String>,
    pub unity_cn_key: Option<String>,
    pub xor_key: u8,
    pub continue_on_error: bool,
    /// Maximum objects retained in one session; metadata budgets scale with it.
    pub maximum_assets: usize,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            game: "Normal".into(),
            unity_version: None,
            unity_cn_key: None,
            xor_key: 0,
            continue_on_error: false,
            maximum_assets: 1_000_000,
        }
    }
}

impl LoadOptions {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=10_000_000).contains(&self.maximum_assets),
            "asset limit must be between 1 and 10000000"
        );
        crate::games::validate_profile(&self.game)?;
        crate::session::backend_options(self)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExportMode {
    #[default]
    Convert,
    Raw,
    Dump,
    Json,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GroupMode {
    #[default]
    ByType,
    ByContainer,
    BySource,
    None,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Bmp,
    Tga,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    FbxAscii,
    FbxBinary,
    Obj,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapFormat {
    Json,
    Xml,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ExportOptions {
    pub mode: ExportMode,
    pub group: GroupMode,
    pub image_format: ImageFormat,
    pub overwrite: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct OperationReport {
    pub written: Vec<PathBuf>,
    pub skipped: Vec<String>,
    pub failed: Vec<String>,
}

impl OperationReport {
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    Text(String),
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Mesh {
        vertices: Vec<[f32; 3]>,
        indices: Vec<u32>,
        /// Source normals, in the same coordinate system as the vertices.
        normals: Option<Vec<[f32; 3]>>,
        /// Source per-vertex RGBA; absent channels are never synthesized.
        colors: Option<Vec<[f32; 4]>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SceneNode {
    pub key: AssetKey,
    pub name: String,
    pub parent: Option<AssetKey>,
}

include!("classes.rs");
