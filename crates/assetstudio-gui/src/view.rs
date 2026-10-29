use std::collections::HashSet;

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

#[derive(Default)]
pub struct Filters {
    pub name: String,
    pub container: String,
    pub class: Option<String>,
    pub regex: bool,
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
    pub original_triangles: usize,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl MeshPreview {
    pub fn new(vertices: Vec<[f32; 3]>, indices: Vec<u32>) -> Result<Self, String> {
        if vertices.is_empty() {
            return Err("This mesh has no vertices.".to_owned());
        }
        if !indices.len().is_multiple_of(3) {
            return Err("The mesh triangle index count is invalid.".to_owned());
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
            original_triangles,
            yaw: -0.5,
            pitch: 0.25,
            zoom: 1.0,
        })
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn is_simplified(&self) -> bool {
        self.triangles.len() < self.original_triangles
    }

    pub fn lines(&self, rect: eframe::egui::Rect) -> Vec<[eframe::egui::Pos2; 2]> {
        let (sine_yaw, cosine_yaw) = self.yaw.sin_cos();
        let (sine_pitch, cosine_pitch) = self.pitch.sin_cos();
        let scale = rect.width().min(rect.height()) * 0.32 * self.zoom;
        let projected: Vec<_> = self
            .vertices
            .iter()
            .map(|vertex| {
                let x = vertex[0] * cosine_yaw + vertex[2] * sine_yaw;
                let z = -vertex[0] * sine_yaw + vertex[2] * cosine_yaw;
                let y = vertex[1] * cosine_pitch - z * sine_pitch;
                rect.center() + eframe::egui::vec2(x * scale, -y * scale)
            })
            .collect();
        let mut lines = Vec::with_capacity(self.triangles.len() * 3);
        for triangle in &self.triangles {
            lines.push([projected[triangle[0]], projected[triangle[1]]]);
            lines.push([projected[triangle[1]], projected[triangle[2]]]);
            lines.push([projected[triangle[2]], projected[triangle[0]]]);
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let lines = mesh.lines(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(400.0, 300.0),
        ));
        assert_eq!(lines.len(), 3);
        assert!(
            lines
                .iter()
                .flatten()
                .all(|point| point.x.is_finite() && point.y.is_finite())
        );
    }
}
