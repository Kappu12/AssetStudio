//! Bounded orthographic mesh projection and flat-shaded preview faces.

use eframe::egui::{Pos2, Rect};

const MAX_VERTICES: usize = 1_000_000;
const MAX_FACES: usize = 10_000;

#[derive(Clone, Copy, Debug)]
pub struct MeshCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    /// Screen-pixel displacement from the preview rectangle's center.
    pub pan: [f32; 2],
}

impl Default for MeshCamera {
    fn default() -> Self {
        Self {
            yaw: -0.5,
            pitch: 0.25,
            zoom: 1.0,
            pan: [0.0, 0.0],
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProjectedFace {
    pub points: [Pos2; 3],
    pub depth: f32,
    pub brightness: f32,
}

/// Projects only referenced vertices. The existing mesh preview provides the
/// already-centered/scaled vertices and its bounded triangle sample.
pub fn project_faces(
    vertices: &[[f32; 3]],
    triangles: &[[usize; 3]],
    rect: Rect,
    camera: MeshCamera,
) -> Result<Vec<ProjectedFace>, String> {
    if vertices.len() > MAX_VERTICES || triangles.len() > MAX_FACES {
        return Err("The mesh exceeds the preview detail limit.".into());
    }
    if !camera.yaw.is_finite()
        || !camera.pitch.is_finite()
        || !camera.zoom.is_finite()
        || camera.zoom <= 0.0
        || camera.zoom > 128.0
        || camera.pan.iter().any(|v| !v.is_finite())
        || [
            rect.min.x,
            rect.min.y,
            rect.max.x,
            rect.max.y,
            rect.width(),
            rect.height(),
        ]
        .iter()
        .any(|v| !v.is_finite())
        || rect.width() <= 0.0
        || rect.height() <= 0.0
    {
        return Err("The mesh camera or preview area is invalid.".into());
    }
    let (sy, cy) = f64::from(camera.yaw).sin_cos();
    let (sp, cp) = f64::from(camera.pitch).sin_cos();
    let scale = f64::from(rect.width().min(rect.height())) * 0.32 * f64::from(camera.zoom);
    let center = [
        (f64::from(rect.min.x) + f64::from(rect.max.x)) * 0.5 + f64::from(camera.pan[0]),
        (f64::from(rect.min.y) + f64::from(rect.max.y)) * 0.5 + f64::from(camera.pan[1]),
    ];
    let mut faces = Vec::new();
    faces
        .try_reserve_exact(triangles.len())
        .map_err(|_| "Cannot allocate mesh preview faces.".to_owned())?;
    for triangle in triangles {
        let mut rotated = [[0_f64; 3]; 3];
        for (slot, index) in rotated.iter_mut().zip(triangle) {
            let vertex = vertices
                .get(*index)
                .ok_or("Mesh preview references a missing vertex.")?;
            if vertex.iter().any(|v| !v.is_finite()) {
                return Err("Mesh preview contains non-finite coordinates.".into());
            }
            let x = f64::from(vertex[0]) * cy + f64::from(vertex[2]) * sy;
            let z = -f64::from(vertex[0]) * sy + f64::from(vertex[2]) * cy;
            *slot = [
                x,
                f64::from(vertex[1]) * cp - z * sp,
                f64::from(vertex[1]) * sp + z * cp,
            ];
        }
        let mut points = [Pos2::ZERO; 3];
        for (point, vertex) in points.iter_mut().zip(rotated) {
            *point = Pos2::new(
                (center[0] + vertex[0] * scale) as f32,
                (center[1] - vertex[1] * scale) as f32,
            );
            if !point.x.is_finite() || !point.y.is_finite() {
                return Err("Mesh projection exceeds the display coordinate range.".into());
            }
        }
        let a = std::array::from_fn::<_, 3, _>(|axis| rotated[1][axis] - rotated[0][axis]);
        let b = std::array::from_fn::<_, 3, _>(|axis| rotated[2][axis] - rotated[0][axis]);
        let normal = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let length = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
        let diffuse = if length > 0.0 {
            ((normal[0] * -0.4 + normal[1] * 0.6 + normal[2]) / length / 1.52_f64.sqrt())
                .abs()
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let depth = ((rotated[0][2] + rotated[1][2] + rotated[2][2]) / 3.0) as f32;
        if !depth.is_finite() {
            return Err("Mesh preview depth exceeds the display coordinate range.".into());
        }
        faces.push(ProjectedFace {
            points,
            depth,
            brightness: (0.22 + 0.78 * diffuse) as f32,
        });
    }
    // View direction is from positive Z; distant faces must be painted first.
    faces.sort_by(|left, right| left.depth.total_cmp(&right.depth));
    Ok(faces)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect() -> Rect {
        Rect::from_min_size(Pos2::ZERO, eframe::egui::vec2(200.0, 200.0))
    }
    #[test]
    fn faces_are_depth_sorted_lit_and_pan_is_in_screen_pixels() {
        let vertices = [
            [-1.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [-1.0, 0.0, -1.0],
            [1.0, 0.0, -1.0],
            [0.0, 1.0, -1.0],
        ];
        let camera = MeshCamera {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
            pan: [10.0, -20.0],
        };
        let faces = project_faces(&vertices, &[[0, 1, 2], [3, 4, 5]], rect(), camera).unwrap();
        assert_eq!(faces.len(), 2);
        assert!(faces[0].depth < faces[1].depth);
        assert_eq!(faces[0].points[0], Pos2::new(46.0, 80.0));
        assert_eq!(faces[0].points[2], Pos2::new(110.0, 16.0));
        assert!(
            faces
                .iter()
                .all(|face| (0.22..=1.0).contains(&face.brightness))
        );
    }
    #[test]
    fn malformed_indices_coordinates_and_camera_are_errors() {
        assert!(project_faces(&[[0.0; 3]], &[[0, 1, 0]], rect(), MeshCamera::default()).is_err());
        assert!(
            project_faces(
                &[[f32::NAN, 0.0, 0.0]],
                &[[0, 0, 0]],
                rect(),
                MeshCamera::default()
            )
            .is_err()
        );
        assert!(
            project_faces(
                &[[0.0; 3]],
                &[[0, 0, 0]],
                rect(),
                MeshCamera {
                    zoom: f32::INFINITY,
                    ..MeshCamera::default()
                }
            )
            .is_err()
        );
        assert!(
            project_faces(
                &[[0.0; 3]],
                &[[0, 0, 0]],
                Rect::NOTHING,
                MeshCamera::default()
            )
            .is_err()
        );
    }
}
