//! Complete serialized MHY clips with independent C++ and baseline DLL samples.
use assetstudio_core::{ExportOptions, LoadOptions, Session};
use std::{fs, path::Path};
use unity_rs_core::{
    animation_clip::AnimationClipReadLimits,
    animation_graph::{AnimationGraphLimits, build_animation_graph},
    game_variant::GameVariant,
    loader::{AssetCollection, AssetLoadOptions},
    model_animation::{ModelAnimationLimits, build_model_animations},
    model_ir::{ModelIrLimits, build_model_ir},
    scene_hierarchy::{SceneHierarchyLimits, build_scene_hierarchy},
};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/animation-mhy")
        .join(name)
}

// Read only vector key values from the emitted YAML; slopes and weights must
// not accidentally satisfy the sample comparisons.
fn yaml_values(yaml: &str, field: &str, width: usize) -> Vec<Vec<f32>> {
    let (_, section) = yaml.split_once(&format!("  {field}:\n")).unwrap();
    let section = section.split("\n  m_").next().unwrap();
    let lines: Vec<_> = section.lines().collect();
    let mut values = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() == "value:" {
            let sample = lines[index + 1..index + 1 + width]
                .iter()
                .map(|line| line.split_once(':').unwrap().1.trim().parse().unwrap())
                .collect();
            values.push(sample);
        }
    }
    values
}

#[test]
fn serialized_mhy_samples_match_every_original_dll_value_and_export_anim() {
    for name in ["mhy_variable", "mhy_mixed_tracks", "mhy_multibone"] {
        let collection = AssetCollection::load_path_with_options(
            fixture(&format!("{name}.assets")),
            AssetLoadOptions {
                game_variant: GameVariant::HonkaiImpact3Pre,
                ..Default::default()
            },
        )
        .unwrap();
        let object = collection.serialized_files()[0]
            .file
            .objects
            .iter()
            .position(|object| object.class_id == 74)
            .unwrap();
        let clip = unity_rs_core::animation_clip::read_animation_clip(
            &collection.serialized_files()[0].file,
            object,
            AnimationClipReadLimits::default(),
        )
        .unwrap();
        let decoded = clip
            .muscle_clip
            .as_ref()
            .unwrap()
            .clip
            .mhy_acl
            .as_ref()
            .unwrap()
            .decode(Default::default())
            .unwrap();
        let oracle = fs::read_to_string(fixture(&format!("{name}.dll.csv"))).unwrap();
        let rows: Vec<Vec<f32>> = oracle
            .lines()
            .map(|line| {
                line.split(',')
                    .map(|value| value.parse().unwrap())
                    .collect()
            })
            .collect();
        assert_eq!(decoded.times.len(), rows[1].len());
        assert_eq!(decoded.values.len(), rows[2].len());
        for (actual, expected) in decoded.times.iter().zip(&rows[1]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        for (index, (actual, expected)) in decoded.values.iter().zip(&rows[2]).enumerate() {
            assert!(
                (actual - expected).abs() < 2e-6 * expected.abs().max(1.0),
                "{name} value {index}"
            );
        }
        let session = Session::open(
            &fixture(&format!("{name}.assets")),
            &LoadOptions {
                game: "BH3Pre".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let key = session
            .assets()
            .iter()
            .find(|asset| asset.class_id == 74)
            .unwrap()
            .key;
        let output = tempfile::tempdir().unwrap();
        let report = session
            .export(&[key], output.path(), &ExportOptions::default())
            .unwrap();
        assert!(report.failed.is_empty(), "{name}: {:?}", report.failed);
        assert_eq!(report.written.len(), 1);
        let yaml = fs::read_to_string(&report.written[0]).unwrap();
        assert!(yaml.contains("m_RotationCurves:"));
        assert!(yaml.contains("m_PositionCurves:"));
        assert!(yaml.contains("m_ScaleCurves:"));
        for (field, width, offset) in [
            ("m_RotationCurves", 4, 0),
            ("m_PositionCurves", 3, 4),
            ("m_ScaleCurves", 3, 7),
        ] {
            let values = yaml_values(&yaml, field, width);
            let bones = decoded.following_curve_offset as usize / 10;
            for bone in 0..bones {
                for frame in 0..decoded.times.len() {
                    let start = frame * bones * 10 + bone * 10 + offset;
                    for (actual, expected) in values[bone * decoded.times.len() + frame]
                        .iter()
                        .zip(&rows[2][start..start + width])
                    {
                        assert!(
                            (actual - expected).abs() < 1e-5 * expected.abs().max(1.0),
                            "{name} {field} bone {bone} frame {frame}"
                        );
                    }
                }
            }
            if name == "mhy_mixed_tracks" && offset == 4 {
                assert_eq!(
                    &values[decoded.times.len()..],
                    &[vec![10.0, 20.0, 30.0], vec![40.0, 50.0, 60.0]]
                );
            }
            if name == "mhy_mixed_tracks" && offset == 7 {
                assert_eq!(
                    &values[decoded.times.len()..],
                    &[vec![2.0, 3.0, 4.0], vec![2.0, 3.0, 4.0]]
                );
            }
        }
        if name != "mhy_variable" {
            assert!(yaml.contains("path: \"Child\""), "{yaml}");
            let hierarchy =
                build_scene_hierarchy(&collection, SceneHierarchyLimits::default()).unwrap();
            let model = build_model_ir(&collection, &hierarchy, ModelIrLimits::default()).unwrap();
            let graph =
                build_animation_graph(&collection, &hierarchy, AnimationGraphLimits::default())
                    .unwrap();
            let animation = build_model_animations(
                &collection,
                &model,
                &graph,
                ModelAnimationLimits::default(),
            )
            .unwrap();
            assert_eq!(animation.clips.len(), 1);
            assert_eq!(
                animation.clips[0].tracks.len(),
                if name == "mhy_multibone" { 3 } else { 2 }
            );
            for (bone, track) in animation.clips[0].tracks.iter().enumerate() {
                if name == "mhy_mixed_tracks" && track.node.path_id == 2 {
                    assert_eq!(track.translations[0].value, [-10.0, 20.0, 30.0]);
                    assert_eq!(track.translations[1].value, [-40.0, 50.0, 60.0]);
                    assert_eq!(track.scalings[0].value, [2.0, 3.0, 4.0]);
                    continue;
                }
                let width = decoded.following_curve_offset as usize;
                assert_eq!(track.translations.len(), decoded.times.len());
                assert_eq!(track.rotations.len(), decoded.times.len());
                assert_eq!(track.scalings.len(), decoded.times.len());
                for frame in 0..decoded.times.len() {
                    let sample = &rows[2][frame * width + bone * 10..];
                    for (actual, expected) in track.translations[frame]
                        .value
                        .iter()
                        .zip([-sample[4], sample[5], sample[6]])
                    {
                        assert!((actual - expected).abs() < 1e-5 * expected.abs().max(1.0));
                    }
                    for (actual, expected) in track.scalings[frame].value.iter().zip(&sample[7..10])
                    {
                        assert!((actual - expected).abs() < 1e-5 * expected.abs().max(1.0));
                    }
                }
            }
        }
    }
}
