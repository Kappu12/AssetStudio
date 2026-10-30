//! Bounded orthographic mesh projection and flat-shaded preview faces.

use eframe::egui::{self, Color32, Pos2, Rect};

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
    /// Present only when an explicitly selected normal channel was supplied.
    pub vertex_brightness: Option<[f32; 3]>,
    /// Unmodified per-vertex source values. Display clipping happens at paint.
    pub vertex_colors: Option<[[f32; 4]; 3]>,
}

#[derive(Default, Clone, Copy)]
pub struct MeshAttributes<'a> {
    pub normals: Option<&'a [[f32; 3]]>,
    pub colors: Option<&'a [[f32; 4]]>,
}

/// Projects only referenced vertices. The existing mesh preview provides the
/// already-centered/scaled vertices and its bounded triangle sample.
pub fn project_faces(
    vertices: &[[f32; 3]],
    triangles: &[[usize; 3]],
    rect: Rect,
    camera: MeshCamera,
) -> Result<Vec<ProjectedFace>, String> {
    project_faces_with_attributes(vertices, triangles, rect, camera, MeshAttributes::default())
}

pub fn project_faces_with_attributes(
    vertices: &[[f32; 3]],
    triangles: &[[usize; 3]],
    rect: Rect,
    camera: MeshCamera,
    attributes: MeshAttributes<'_>,
) -> Result<Vec<ProjectedFace>, String> {
    if vertices.len() > MAX_VERTICES || triangles.len() > MAX_FACES {
        return Err("The mesh exceeds the preview detail limit.".into());
    }
    if attributes.normals.is_some_and(|values| {
        values.len() != vertices.len() || values.iter().flatten().any(|v| !v.is_finite())
    }) || attributes.colors.is_some_and(|values| {
        values.len() != vertices.len() || values.iter().flatten().any(|v| !v.is_finite())
    }) {
        return Err("Mesh preview attributes are incomplete or non-finite.".into());
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
    let rotate = |vertex: [f32; 3]| {
        let x = f64::from(vertex[0]) * cy + f64::from(vertex[2]) * sy;
        let z = -f64::from(vertex[0]) * sy + f64::from(vertex[2]) * cy;
        [
            x,
            f64::from(vertex[1]) * cp - z * sp,
            f64::from(vertex[1]) * sp + z * cp,
        ]
    };
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
            *slot = rotate(*vertex);
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
        let depth = ((rotated[0][2] + rotated[1][2] + rotated[2][2]) / 3.0) as f32;
        if !depth.is_finite() {
            return Err("Mesh preview depth exceeds the display coordinate range.".into());
        }
        faces.push(ProjectedFace {
            points,
            depth,
            brightness: brightness(normal),
            vertex_brightness: attributes
                .normals
                .map(|normals| triangle.map(|index| brightness(rotate(normals[index])))),
            vertex_colors: attributes
                .colors
                .map(|colors| triangle.map(|index| colors[index])),
        });
    }
    // View direction is from positive Z; distant faces must be painted first.
    faces.sort_by(|left, right| left.depth.total_cmp(&right.depth));
    Ok(faces)
}

fn brightness(normal: [f64; 3]) -> f32 {
    let length = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
    let diffuse = if length > 0.0 {
        ((normal[0] * -0.4 + normal[1] * 0.6 + normal[2]) / length / 1.52_f64.sqrt())
            .abs()
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    (0.22 + 0.78 * diffuse) as f32
}

/// One independent triangle preserves per-vertex colors after depth sorting.
/// The renderer interpolates these values; averaging the face would discard
/// actual source color and smooth-normal variation.
pub fn shaded_mesh(face: &ProjectedFace, base: Color32) -> egui::Mesh {
    let colors = if let Some(colors) = face.vertex_colors {
        colors.map(|value| {
            let [r, g, b, a] =
                value.map(|component| (component.clamp(0.0, 1.0) * 255.0).round() as u8);
            Color32::from_rgba_unmultiplied(r, g, b, a)
        })
    } else if let Some(light) = face.vertex_brightness {
        light.map(|brightness| base.gamma_multiply(brightness))
    } else {
        [base.gamma_multiply(face.brightness); 3]
    };
    egui::Mesh {
        vertices: face
            .points
            .into_iter()
            .zip(colors)
            .map(|(pos, color)| egui::epaint::Vertex {
                pos,
                uv: egui::epaint::WHITE_UV,
                color,
            })
            .collect(),
        indices: vec![0, 1, 2],
        ..Default::default()
    }
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

    #[test]
    fn actual_source_normals_are_rotated_and_preserve_vertex_lighting() {
        let vertices = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0; 3]];
        let camera = MeshCamera {
            yaw: 0.0,
            pitch: 0.0,
            ..MeshCamera::default()
        };
        let faces = project_faces_with_attributes(
            &vertices,
            &[[0, 1, 2]],
            rect(),
            camera,
            MeshAttributes {
                normals: Some(&normals),
                colors: None,
            },
        )
        .unwrap();
        let light = faces[0].vertex_brightness.unwrap();
        assert!((light[0] - faces[0].brightness).abs() < 1e-6);
        assert!(light[0] > light[1] && light[1] > light[2]);
        assert_eq!(light[2], 0.22);
        let turned = project_faces_with_attributes(
            &vertices,
            &[[0, 1, 2]],
            rect(),
            MeshCamera {
                yaw: std::f32::consts::FRAC_PI_2,
                ..camera
            },
            MeshAttributes {
                normals: Some(&normals),
                colors: None,
            },
        )
        .unwrap();
        assert!((turned[0].vertex_brightness.unwrap()[0] - light[1]).abs() < 1e-5);
    }

    #[test]
    fn colored_triangles_keep_individual_rgba_values_in_the_paint_mesh() {
        let vertices = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let colors = [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 0.5],
            [-1.0, 0.0, 2.0, 0.0],
        ];
        let faces = project_faces_with_attributes(
            &vertices,
            &[[0, 1, 2]],
            rect(),
            MeshCamera::default(),
            MeshAttributes {
                normals: None,
                colors: Some(&colors),
            },
        )
        .unwrap();
        assert_eq!(faces[0].vertex_colors, Some(colors));
        let mesh = shaded_mesh(&faces[0], Color32::WHITE);
        assert_eq!(mesh.indices, [0, 1, 2]);
        assert_eq!(mesh.vertices[0].color, Color32::RED);
        assert_eq!(
            mesh.vertices[1].color,
            Color32::from_rgba_unmultiplied(0, 255, 0, 128)
        );
        assert_eq!(mesh.vertices[2].color, Color32::TRANSPARENT);
        assert_ne!(mesh.vertices[0].color, mesh.vertices[1].color);
    }

    #[test]
    fn incomplete_and_nonfinite_source_channels_are_rejected() {
        let vertices = [[0.0; 3]; 3];
        for attributes in [
            MeshAttributes {
                normals: Some(&[[0.0; 3]; 2]),
                colors: None,
            },
            MeshAttributes {
                normals: Some(&[[f32::NAN; 3]; 3]),
                colors: None,
            },
            MeshAttributes {
                normals: None,
                colors: Some(&[[0.0; 4]; 2]),
            },
            MeshAttributes {
                normals: None,
                colors: Some(&[[f32::INFINITY; 4]; 3]),
            },
        ] {
            assert!(
                project_faces_with_attributes(
                    &vertices,
                    &[[0, 1, 2]],
                    rect(),
                    MeshCamera::default(),
                    attributes
                )
                .is_err()
            );
        }
    }
}
