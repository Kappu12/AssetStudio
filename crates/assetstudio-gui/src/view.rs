use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use assetstudio_core::{AssetFilter, AssetKey, AssetRecord};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortColumn {
    #[default]
    Name,
    Class,
    Size,
    Container,
    Source,
    PathId,
}

#[derive(Clone, Default)]
pub struct Filters {
    pub name: String,
    pub container: String,
    pub class: Option<String>,
    pub regex: bool,
}

pub struct Catalog {
    pub assets: Arc<Vec<AssetRecord>>,
    pub classes: BTreeMap<String, usize>,
    pub by_key: HashMap<AssetKey, usize>,
}

impl Catalog {
    /// Materialized by the load worker, including all copying and indexes.
    #[cfg(test)]
    pub fn prepare(records: &[AssetRecord]) -> Result<Self, String> {
        Self::prepare_with_progress(records, |_, _| {})
    }

    pub fn prepare_with_progress(
        records: &[AssetRecord],
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Self, String> {
        let mut assets = Vec::new();
        assets
            .try_reserve_exact(records.len())
            .map_err(|error| error.to_string())?;
        let mut by_key = HashMap::new();
        by_key
            .try_reserve(records.len())
            .map_err(|error| error.to_string())?;
        let mut classes = BTreeMap::new();
        let copy = |value: &str| -> Result<String, String> {
            let mut output = String::new();
            output
                .try_reserve_exact(value.len())
                .map_err(|error| error.to_string())?;
            output.push_str(value);
            Ok(output)
        };
        progress(0, records.len());
        for (index, record) in records.iter().enumerate() {
            if index % 4096 == 0 {
                progress(index, records.len());
            }
            if by_key.insert(record.key, assets.len()).is_some() {
                return Err("The asset catalog contains duplicate object identities.".to_owned());
            }
            *classes.entry(copy(&record.class_name)?).or_default() += 1;
            assets.push(AssetRecord {
                key: record.key,
                class_id: record.class_id,
                class_name: copy(&record.class_name)?,
                name: copy(&record.name)?,
                container: copy(&record.container)?,
                source: copy(&record.source)?,
                size: record.size,
            });
        }
        progress(records.len(), records.len());
        Ok(Self {
            assets: Arc::new(assets),
            classes,
            by_key,
        })
    }
}

pub fn visible_rows(
    assets: &[AssetRecord],
    filters: &Filters,
    sort: SortColumn,
    descending: bool,
) -> Result<Vec<usize>, String> {
    let patterns = |value: &str| {
        if value.is_empty() {
            Vec::new()
        } else if filters.regex {
            vec![value.to_owned()]
        } else {
            vec![regex::escape(value)]
        }
    };
    let classes = filters.class.iter().cloned().collect::<Vec<_>>();
    let filter = AssetFilter::new(
        &classes,
        &patterns(&filters.name),
        &patterns(&filters.container),
    )
    .map_err(|error| error.to_string())?;
    let mut rows: Vec<_> = assets
        .iter()
        .enumerate()
        .filter_map(|(index, asset)| filter.matches(asset).then_some(index))
        .collect();
    rows.sort_by(|&left, &right| {
        let a = &assets[left];
        let b = &assets[right];
        let order = match sort {
            SortColumn::Name => a.name.cmp(&b.name),
            SortColumn::Class => a.class_name.cmp(&b.class_name),
            SortColumn::Size => a.size.cmp(&b.size),
            SortColumn::Container => a.container.cmp(&b.container),
            SortColumn::Source => a.source.cmp(&b.source),
            SortColumn::PathId => a.key.path_id.cmp(&b.key.path_id),
        };
        let order = order.then_with(|| a.key.cmp(&b.key));
        if descending { order.reverse() } else { order }
    });
    Ok(rows)
}

pub fn select_row(
    selected: &mut HashSet<AssetKey>,
    primary: &mut Option<AssetKey>,
    anchor: &mut Option<AssetKey>,
    ordered_keys: &[AssetKey],
    key: AssetKey,
    toggle: bool,
    range: bool,
) {
    if range
        && let Some(start) =
            anchor.and_then(|value| ordered_keys.iter().position(|key| *key == value))
        && let Some(end) = ordered_keys.iter().position(|value| *value == key)
    {
        if !toggle {
            selected.clear();
        }
        selected.extend(
            ordered_keys[start.min(end)..=start.max(end)]
                .iter()
                .copied(),
        );
        *primary = Some(key);
        return;
    }
    if toggle {
        if !selected.remove(&key) {
            selected.insert(key);
            *primary = Some(key);
        } else if *primary == Some(key) {
            // Choose deterministically; HashSet iteration is deliberately unordered.
            *primary = ordered_keys
                .iter()
                .copied()
                .find(|key| selected.contains(key));
        }
    } else {
        selected.clear();
        selected.insert(key);
        *primary = Some(key);
    }
    *anchor = Some(key);
}

pub fn preview_is_current(
    loaded_generation: u64,
    result_generation: u64,
    selected: Option<AssetKey>,
    result_key: AssetKey,
) -> bool {
    loaded_generation == result_generation && selected == Some(result_key)
}

pub fn byte_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1} MiB", bytes as f64 / 1_048_576.0),
        _ => format!("{:.2} GiB", bytes as f64 / 1_073_741_824.0),
    }
}

pub struct MeshPreview {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[usize; 3]>,
    source_normals: Option<Vec<[f32; 3]>>,
    calculated_normals: Vec<[f32; 3]>,
    colors: Option<Vec<[f32; 4]>>,
    pub original_triangles: usize,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub pan: [f32; 2],
    pub mode: MeshMode,
    pub shading: MeshShading,
    previous_normal_shading: MeshShading,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MeshMode {
    Shaded,
    Wireframe,
    Combined,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeshShading {
    #[default]
    Flat,
    CalculatedNormals,
    SourceNormals,
    VertexColors,
}

impl MeshShading {
    pub fn label(self) -> &'static str {
        match self {
            Self::Flat => "Flat",
            Self::CalculatedNormals => "Calculated normals",
            Self::SourceNormals => "Source normals",
            Self::VertexColors => "Vertex colors",
        }
    }

    pub fn preference(self) -> &'static str {
        match self {
            Self::Flat => "Flat",
            Self::CalculatedNormals => "CalculatedNormals",
            Self::SourceNormals => "SourceNormals",
            Self::VertexColors => "VertexColors",
        }
    }

    pub fn from_preference(value: &str) -> Result<Self, String> {
        match value {
            "Flat" => Ok(Self::Flat),
            "CalculatedNormals" => Ok(Self::CalculatedNormals),
            "SourceNormals" => Ok(Self::SourceNormals),
            "VertexColors" => Ok(Self::VertexColors),
            _ => Err("Saved mesh shading mode is unknown.".to_owned()),
        }
    }
}

impl MeshPreview {
    pub fn new(vertices: Vec<[f32; 3]>, indices: Vec<u32>) -> Result<Self, String> {
        Self::with_attributes(vertices, indices, None, None)
    }

    pub fn with_attributes(
        vertices: Vec<[f32; 3]>,
        indices: Vec<u32>,
        source_normals: Option<Vec<[f32; 3]>>,
        colors: Option<Vec<[f32; 4]>>,
    ) -> Result<Self, String> {
        if vertices.is_empty() {
            return Err("This mesh has no vertices.".to_owned());
        }
        if vertices.len() > 100_000 || indices.len() > 300_000 {
            return Err("The mesh exceeds the preview detail limit.".to_owned());
        }
        if !indices.len().is_multiple_of(3) {
            return Err("The mesh triangle index count is invalid.".to_owned());
        }
        if source_normals.as_ref().is_some_and(|values| {
            values.len() != vertices.len() || values.iter().flatten().any(|v| !v.is_finite())
        }) || colors.as_ref().is_some_and(|values| {
            values.len() != vertices.len() || values.iter().flatten().any(|v| !v.is_finite())
        }) {
            return Err("Mesh source attributes are incomplete or non-finite.".to_owned());
        }
        let mut minimum = [f64::INFINITY; 3];
        let mut maximum = [f64::NEG_INFINITY; 3];
        for vertex in &vertices {
            for axis in 0..3 {
                if !vertex[axis].is_finite() {
                    return Err("The mesh contains non-finite coordinates.".to_owned());
                }
                minimum[axis] = minimum[axis].min(f64::from(vertex[axis]));
                maximum[axis] = maximum[axis].max(f64::from(vertex[axis]));
            }
        }
        if indices
            .iter()
            .any(|&index| index as usize >= vertices.len())
        {
            return Err("The mesh references a vertex that does not exist.".to_owned());
        }
        let center =
            std::array::from_fn::<_, 3, _>(|axis| minimum[axis] * 0.5 + maximum[axis] * 0.5);
        let extent = (0..3)
            .map(|axis| maximum[axis] * 0.5 - minimum[axis] * 0.5)
            .fold(0.0_f64, f64::max)
            .max(f64::MIN_POSITIVE);
        let vertices: Vec<_> = vertices
            .into_iter()
            .map(|vertex| {
                std::array::from_fn(|axis| {
                    ((f64::from(vertex[axis]) - center[axis]) / extent) as f32
                })
            })
            .collect();
        if vertices
            .iter()
            .flatten()
            .any(|coordinate| !coordinate.is_finite())
        {
            return Err("The mesh coordinates cannot be displayed at this scale.".to_owned());
        }
        // Average unit face normals across the complete bounded source mesh.
        // Sampling first would change lighting when a preview reduces detail.
        let calculated_normals = calculated_normals(&vertices, &indices)?;
        let original_triangles = indices.len() / 3;
        // Spread the preview over the complete mesh, rather than truncating one end.
        let stride = original_triangles.div_ceil(10_000).max(1);
        let triangles = indices
            .as_chunks::<3>()
            .0
            .iter()
            .step_by(stride)
            .map(|triangle| {
                [
                    triangle[0] as usize,
                    triangle[1] as usize,
                    triangle[2] as usize,
                ]
            })
            .collect();
        Ok(Self {
            vertices,
            triangles,
            source_normals,
            calculated_normals,
            colors,
            original_triangles,
            yaw: -0.5,
            pitch: 0.25,
            zoom: 1.0,
            pan: [0.0; 2],
            mode: MeshMode::Shaded,
            shading: MeshShading::Flat,
            previous_normal_shading: MeshShading::Flat,
        })
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn is_simplified(&self) -> bool {
        self.triangles.len() < self.original_triangles
    }

    pub fn supports_shading(&self, mode: MeshShading) -> bool {
        match mode {
            MeshShading::Flat | MeshShading::CalculatedNormals => true,
            MeshShading::SourceNormals => self.source_normals.is_some(),
            MeshShading::VertexColors => self.colors.is_some(),
        }
    }

    pub fn set_shading(&mut self, mode: MeshShading) -> bool {
        if !self.supports_shading(mode) {
            return false;
        }
        self.shading = mode;
        if mode != MeshShading::VertexColors {
            self.previous_normal_shading = mode;
        }
        true
    }

    pub fn cycle_wireframe(&mut self) {
        self.mode = match self.mode {
            MeshMode::Shaded => MeshMode::Wireframe,
            MeshMode::Wireframe => MeshMode::Combined,
            MeshMode::Combined => MeshMode::Shaded,
        };
    }

    pub fn toggle_vertex_colors(&mut self) -> bool {
        self.set_shading(if self.shading == MeshShading::VertexColors {
            self.previous_normal_shading
        } else {
            MeshShading::VertexColors
        })
    }

    pub fn toggle_normals(&mut self) -> bool {
        let next = if self.previous_normal_shading == MeshShading::SourceNormals {
            MeshShading::CalculatedNormals
        } else {
            MeshShading::SourceNormals
        };
        if !self.supports_shading(next) {
            return false;
        }
        self.previous_normal_shading = next;
        if self.shading != MeshShading::VertexColors {
            self.shading = next;
        }
        true
    }

    pub fn faces(
        &self,
        rect: eframe::egui::Rect,
    ) -> Result<Vec<crate::preview_geometry::ProjectedFace>, String> {
        let camera = crate::preview_geometry::MeshCamera {
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
            pan: self.pan,
        };
        if self.shading == MeshShading::Flat {
            return crate::preview_geometry::project_faces(
                &self.vertices,
                &self.triangles,
                rect,
                camera,
            );
        }
        if !self.supports_shading(self.shading) {
            return Err("This mesh has no source data for the selected shading mode.".to_owned());
        }
        let attributes = crate::preview_geometry::MeshAttributes {
            normals: match self.shading {
                MeshShading::CalculatedNormals => Some(self.calculated_normals.as_slice()),
                MeshShading::SourceNormals => self.source_normals.as_deref(),
                _ => None,
            },
            colors: if self.shading == MeshShading::VertexColors {
                self.colors.as_deref()
            } else {
                None
            },
        };
        crate::preview_geometry::project_faces_with_attributes(
            &self.vertices,
            &self.triangles,
            rect,
            camera,
            attributes,
        )
    }

    pub fn reset_camera(&mut self) {
        self.yaw = -0.5;
        self.pitch = 0.25;
        self.zoom = 1.0;
        self.pan = [0.0; 2];
    }
}

fn calculated_normals(vertices: &[[f32; 3]], indices: &[u32]) -> Result<Vec<[f32; 3]>, String> {
    let mut sums = Vec::new();
    sums.try_reserve_exact(vertices.len())
        .map_err(|_| "Cannot allocate calculated preview normals.".to_owned())?;
    sums.resize(vertices.len(), [0_f64; 3]);
    for triangle in indices.as_chunks::<3>().0 {
        let a = vertices[triangle[0] as usize];
        let b = vertices[triangle[1] as usize];
        let c = vertices[triangle[2] as usize];
        let u = std::array::from_fn::<_, 3, _>(|axis| f64::from(b[axis]) - f64::from(a[axis]));
        let v = std::array::from_fn::<_, 3, _>(|axis| f64::from(c[axis]) - f64::from(a[axis]));
        let normal = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = normal.iter().map(|n| n * n).sum::<f64>().sqrt();
        if length == 0.0 {
            continue;
        }
        for &index in triangle {
            for (value, normal) in sums[index as usize].iter_mut().zip(normal) {
                *value += normal / length;
            }
        }
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(vertices.len())
        .map_err(|_| "Cannot allocate calculated preview normals.".to_owned())?;
    for sum in sums {
        let length = sum.iter().map(|n| n * n).sum::<f64>().sqrt();
        result.push(if length > 0.0 {
            sum.map(|n| (n / length) as f32)
        } else {
            [0.0, 1.0, 0.0]
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: i64, name: &str, class: &str) -> AssetRecord {
        AssetRecord {
            key: key(id),
            class_id: 49,
            class_name: class.to_owned(),
            name: name.to_owned(),
            container: "folder/file.txt".to_owned(),
            source: "CAB-file".to_owned(),
            size: 16,
        }
    }

    #[test]
    fn catalog_preparation_owns_metadata_and_builds_correct_indexes() {
        let mut source = vec![
            record(2, "second", "TextAsset"),
            record(1, "first", "TextAsset"),
        ];
        let catalog = Catalog::prepare(&source).unwrap();
        assert_eq!(catalog.by_key[&key(1)], 1);
        assert_eq!(catalog.classes["TextAsset"], 2);
        source[0].name = "changed after preparation".to_owned();
        assert_eq!(catalog.assets[0].name, "second");
        assert!(Catalog::prepare(&[source[0].clone(), source[0].clone()]).is_err());
    }

    #[test]
    fn filtering_treats_plain_queries_literally_and_reports_invalid_regex() {
        let assets = [
            record(2, "other", "TextAsset"),
            record(1, "a[1]", "TextAsset"),
        ];
        let filters = Filters {
            name: "[1]".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            visible_rows(&assets, &filters, SortColumn::Name, false).unwrap(),
            vec![1]
        );
        let filters = Filters {
            name: "[".to_owned(),
            regex: true,
            ..Default::default()
        };
        assert!(visible_rows(&assets, &filters, SortColumn::Name, false).is_err());
    }

    fn key(id: i64) -> AssetKey {
        AssetKey {
            file_index: 0,
            path_id: id,
        }
    }

    #[test]
    fn shift_selection_uses_display_order_and_keeps_anchor() {
        let keys = [key(3), key(1), key(2)];
        let mut selected = HashSet::new();
        let mut primary = None;
        let mut anchor = None;
        select_row(
            &mut selected,
            &mut primary,
            &mut anchor,
            &keys,
            key(1),
            false,
            false,
        );
        select_row(
            &mut selected,
            &mut primary,
            &mut anchor,
            &keys,
            key(2),
            false,
            true,
        );
        assert_eq!(selected, HashSet::from([key(1), key(2)]));
        assert_eq!(primary, Some(key(2)));
        assert_eq!(anchor, Some(key(1)));
        select_row(
            &mut selected,
            &mut primary,
            &mut anchor,
            &keys,
            key(3),
            false,
            true,
        );
        assert_eq!(selected, HashSet::from([key(3), key(1)]));
    }

    #[test]
    fn removing_primary_selects_first_remaining_visible_key() {
        let keys = [key(3), key(1), key(2)];
        let mut selected = HashSet::from([key(1), key(2)]);
        let mut primary = Some(key(1));
        let mut anchor = None;
        select_row(
            &mut selected,
            &mut primary,
            &mut anchor,
            &keys,
            key(1),
            true,
            false,
        );
        assert_eq!(primary, Some(key(2)));
        assert!(!selected.contains(&key(1)));
    }

    #[test]
    fn preview_cannot_cross_a_load_or_selection_change() {
        assert!(preview_is_current(4, 4, Some(key(1)), key(1)));
        assert!(!preview_is_current(5, 4, Some(key(1)), key(1)));
        assert!(!preview_is_current(4, 4, Some(key(2)), key(1)));
        assert!(!preview_is_current(4, 4, None, key(1)));
    }

    #[test]
    fn mesh_refuses_bad_indices_and_non_finite_coordinates() {
        assert!(MeshPreview::new(vec![[0.0, 0.0, 0.0]], vec![0, 1, 0]).is_err());
        assert!(MeshPreview::new(vec![[f32::NAN, 0.0, 0.0]], vec![]).is_err());
        assert!(MeshPreview::new(vec![[0.0, 0.0, 0.0]], vec![0]).is_err());
    }

    #[test]
    fn mesh_projection_preserves_finite_coordinates_for_extreme_bounds() {
        let mesh = MeshPreview::new(
            vec![
                [-f32::MAX, 0.0, 0.0],
                [f32::MAX, 0.0, 0.0],
                [0.0, f32::MAX, 0.0],
            ],
            vec![0, 1, 2],
        )
        .unwrap();
        let faces = mesh
            .faces(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(400.0, 300.0),
            ))
            .unwrap();
        assert_eq!(faces.len(), 1);
        assert!(
            faces
                .iter()
                .flat_map(|face| face.points)
                .all(|point| point.x.is_finite() && point.y.is_finite())
        );
    }

    #[test]
    fn source_modes_require_real_complete_channels_and_preserve_keyboard_state() {
        let vertices = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut plain = MeshPreview::new(vertices.clone(), vec![0, 1, 2]).unwrap();
        assert!(!plain.set_shading(MeshShading::SourceNormals));
        assert!(!plain.set_shading(MeshShading::VertexColors));
        assert_eq!(plain.shading, MeshShading::Flat);
        assert!(plain.set_shading(MeshShading::CalculatedNormals));
        let mut colored = MeshPreview::with_attributes(
            vertices,
            vec![0, 1, 2],
            Some(vec![[0.0, 0.0, -1.0]; 3]),
            Some(vec![[0.2, 0.4, 0.6, 1.0]; 3]),
        )
        .unwrap();
        assert!(colored.set_shading(MeshShading::SourceNormals));
        assert!(colored.toggle_vertex_colors());
        assert_eq!(colored.shading, MeshShading::VertexColors);
        assert!(colored.toggle_normals());
        assert_eq!(colored.shading, MeshShading::VertexColors);
        assert!(colored.toggle_vertex_colors());
        assert_eq!(colored.shading, MeshShading::CalculatedNormals);
        colored.cycle_wireframe();
        assert!(colored.mode == MeshMode::Wireframe);
        colored.cycle_wireframe();
        assert!(colored.mode == MeshMode::Combined);
        colored.cycle_wireframe();
        assert!(colored.mode == MeshMode::Shaded);
    }

    #[test]
    fn calculated_normals_average_complete_triangle_neighborhood_and_ignore_degeneracy() {
        let vertices = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [2.0, 2.0, 2.0],
        ];
        let result = calculated_normals(&vertices, &[0, 1, 2, 0, 3, 1, 0, 0, 0]).unwrap();
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        for index in [0, 1] {
            assert!((result[index][1] - expected).abs() < 1e-6);
            assert!((result[index][2] - expected).abs() < 1e-6);
        }
        assert_eq!(result[2], [0.0, 0.0, 1.0]);
        assert_eq!(result[3], [0.0, 1.0, 0.0]);
        assert_eq!(result[4], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn malformed_attributes_and_excessive_preview_work_fail_before_rendering() {
        let vertices = vec![[0.0; 3]; 3];
        for (normals, colors) in [
            (Some(vec![[0.0; 3]; 2]), None),
            (Some(vec![[f32::NAN; 3]; 3]), None),
            (None, Some(vec![[f32::INFINITY; 4]; 3])),
            (None, Some(vec![[0.0; 4]; 2])),
        ] {
            assert!(
                MeshPreview::with_attributes(vertices.clone(), vec![0, 1, 2], normals, colors)
                    .is_err()
            );
        }
        assert!(MeshPreview::new(vec![[0.0; 3]; 100_001], Vec::new()).is_err());
        assert!(MeshPreview::new(vertices, vec![0; 300_003]).is_err());
    }
}
