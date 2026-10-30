//! Independent upstream C++ vectors for default/constant/padding ACL edge cases.

use std::io::{Cursor, Read};

use unity_rs_core::acl::{
    AclCompressedTracksLimits, AclNativeDecodeLimits, AclTrackType, decode_compressed_tracks,
};
use unity_rs_core::source::Region;

const FIXTURES: &[u8] = include_bytes!("fixtures/acl/edge-cases.zip");
const MAX_MEMBER_BYTES: u64 = 128 * 1024;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024;

fn read_member(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    remaining: &mut u64,
) -> Vec<u8> {
    let entry = archive.by_name(name).expect("required independent fixture");
    let declared = entry.size();
    assert!(declared <= MAX_MEMBER_BYTES && declared <= *remaining);
    let mut bytes = Vec::new();
    entry
        .take(MAX_MEMBER_BYTES + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(u64::try_from(bytes.len()).unwrap(), declared);
    *remaining -= declared;
    bytes
}

#[test]
fn all_96_upstream_edge_cases_match_every_native_sample() {
    assert!(FIXTURES.len() < 750 * 1024);
    let mut archive = zip::ZipArchive::new(Cursor::new(FIXTURES)).unwrap();
    assert_eq!(archive.len(), 193);
    let mut remaining = MAX_TOTAL_BYTES;
    let manifest: serde_json::Value =
        serde_json::from_slice(&read_member(&mut archive, "manifest.json", &mut remaining))
            .unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(
        manifest["acl_commit"],
        "414689d5cff4286a7898487a46dc5e48005d38da"
    );
    assert_eq!(
        manifest["rtm_commit"],
        "d7982f2b2524feeb322f424b29cf43df30b5d5b7"
    );
    let cases = manifest["cases"].as_object().unwrap();
    assert_eq!(cases.len(), 96);
    let limits = AclNativeDecodeLimits {
        compressed: AclCompressedTracksLimits {
            maximum_compressed_bytes: 64 * 1024,
            maximum_tracks: 17,
            maximum_samples_per_track: 37,
            maximum_decompressed_values: 17 * 37 * 10,
        },
        maximum_materialized_bytes: 1024 * 1024,
    };
    let mut compared = 0;
    for tracks in [1_u32, 2, 3, 4, 5, 15, 16, 17] {
        for source_frames in [1_u32, 11, 37] {
            for encoding in 0..4 {
                let name = format!("edge_{tracks}_{source_frames}_{encoding}");
                let case = &cases[&name];
                assert_eq!(case["tracks"], tracks);
                assert_eq!(case["source_frames"], source_frames);
                assert_eq!(case["encoding"], encoding);
                let encoded = read_member(&mut archive, &format!("{name}.acl"), &mut remaining);
                let decoded = decode_compressed_tracks(&Region::from_bytes(encoded), limits)
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                assert_eq!(decoded.header.track_type, AclTrackType::Qvv);
                assert_eq!(decoded.header.num_tracks, tracks);
                assert_eq!(case["stored_frames"], decoded.header.num_samples_per_track);
                assert_eq!(decoded.header.sample_rate(), 30.0);
                let csv = String::from_utf8(read_member(
                    &mut archive,
                    &format!("{name}.csv"),
                    &mut remaining,
                ))
                .unwrap();
                let mut rows = csv.lines();
                assert_eq!(
                    rows.next().unwrap(),
                    format!("{tracks},{},10,30", decoded.header.num_samples_per_track)
                );
                let expected: Vec<f32> = rows
                    .flat_map(|row| row.split(',').map(|value| value.parse::<f32>().unwrap()))
                    .collect();
                assert_eq!(decoded.values.len(), expected.len(), "{name}");
                for (index, (actual, expected)) in decoded.values.iter().zip(expected).enumerate() {
                    assert!(
                        actual.is_finite()
                            && (actual - expected).abs() <= 2.0e-6 * expected.abs().max(1.0),
                        "{name} scalar {index}: {actual} != {expected}"
                    );
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 100_000);
}
