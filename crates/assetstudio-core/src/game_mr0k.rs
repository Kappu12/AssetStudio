//! Expanded-key AES/mr0k compatibility used by the baseline miHoYo bundles.
//!
//! The inverse-round ordering follows AssetStudio/Crypto/AES.cs (Chris Hulbert's
//! credited AES implementation) and Mr0kUtils.cs under the repository MIT license.
//! These algorithms are readers for existing formats, not new encryption APIs.

use anyhow::{Context, Result, ensure};

#[path = "game_mr0k_constants.rs"]
mod constants;
use constants::*;

struct Keys<'a> {
    expanded: &'a [u8; 176],
    block: &'a [u8],
    iv: &'a [u8],
    post: &'a [u8],
}

pub(super) fn decrypt(input: &[u8], profile: &str) -> Result<Vec<u8>> {
    let keys = match profile {
        "BH3PrePre" => Keys {
            expanded: &PACK_EXPANSION_KEY,
            block: &PACK_BLOCK_KEY,
            iv: &[],
            post: &[],
        },
        "TOT" => Keys {
            expanded: &MR0K_EXPANSION_KEY,
            block: &MR0K_BLOCK_KEY,
            iv: &MR0K_INIT_VECTOR,
            post: &TOT_POST_KEY,
        },
        _ => anyhow::bail!("No verified mr0k key configuration for {profile}"),
    };
    decrypt_with_keys(input, &keys)
}

fn decrypt_with_keys(input: &[u8], keys: &Keys<'_>) -> Result<Vec<u8>> {
    ensure!(input.starts_with(b"mr0k"), "Invalid mr0k block signature");
    ensure!(input.len() >= 0x94, "mr0k block header is truncated");
    ensure!(
        !keys.block.is_empty() && keys.iv.len() <= 16,
        "Invalid mr0k key configuration"
    );
    let length = u32::try_from(input.len()).context("mr0k block exceeds 32-bit format size")?;
    if !keys.post.is_empty() {
        ensure!(
            input.len() - 20 >= 0xc00,
            "mr0k post-key region is truncated"
        );
    }
    let mut key1: [u8; 16] = input[4..20].try_into()?;
    let mut key2: [u8; 16] = input[0x74..0x84].try_into()?;
    let mut key3: [u8; 16] = input[0x84..0x94].try_into()?;
    for (byte, iv) in key2.iter_mut().zip(keys.iv) {
        *byte ^= iv;
    }
    decrypt_expanded(&mut key1, keys.expanded);
    decrypt_expanded(&mut key3, keys.expanded);
    for (byte, other) in key1.iter_mut().zip(key3) {
        *byte ^= other;
    }
    let seed1 = u64::from_le_bytes(key2[..8].try_into()?);
    let seed2 = u64::from_le_bytes(key3[..8].try_into()?);
    let seed = seed2 ^ seed1 ^ seed1.wrapping_add(u64::from(length)).wrapping_sub(20);
    let seed = seed.to_le_bytes();
    let encrypted = (((input.len() - 0x94) >> 7) * 16).min(0x400);
    let mut data = super::copy_bytes(input)?;
    data[0x84..0x94].copy_from_slice(&key1);
    for (index, byte) in data[0x94..0x94 + encrypted].iter_mut().enumerate() {
        *byte ^= seed[index % seed.len()] ^ keys.block[index % keys.block.len()];
    }
    data.copy_within(20.., 0);
    data.truncate(data.len() - 20);
    if !keys.post.is_empty() {
        for (index, byte) in data[..0xc00].iter_mut().enumerate() {
            *byte ^= keys.post[index % keys.post.len()];
        }
    }
    Ok(data)
}

/// The on-disk key is already an inverse-mixed decryption schedule. Supplying
/// its first sixteen bytes to an ordinary AES constructor is not equivalent.
fn decrypt_expanded(state: &mut [u8; 16], keys: &[u8; 176]) {
    xor_round(state, keys, 0);
    for round in 1..10 {
        inverse_sub_shift(state);
        inverse_mix(state);
        xor_round(state, keys, round);
    }
    inverse_sub_shift(state);
    xor_round(state, keys, 10);
}

fn xor_round(state: &mut [u8; 16], keys: &[u8; 176], round: usize) {
    for (byte, key) in state.iter_mut().zip(&keys[round * 16..round * 16 + 16]) {
        *byte ^= key;
    }
}

fn inverse_sub_shift(state: &mut [u8; 16]) {
    const ORDER: [usize; 16] = [0, 13, 10, 7, 4, 1, 14, 11, 8, 5, 2, 15, 12, 9, 6, 3];
    let previous = *state;
    for (index, byte) in state.iter_mut().enumerate() {
        *byte = INVERSE_SBOX[usize::from(previous[ORDER[index]])];
    }
}

fn gf_mul(mut value: u8, mut multiplier: u8) -> u8 {
    let mut result = 0;
    for _ in 0..8 {
        if multiplier & 1 != 0 {
            result ^= value;
        }
        value = (value << 1) ^ if value & 0x80 != 0 { 0x1b } else { 0 };
        multiplier >>= 1;
    }
    result
}

fn inverse_mix(state: &mut [u8; 16]) {
    for column in state.as_chunks_mut::<4>().0 {
        let [a, b, c, d] = *column;
        *column = [
            gf_mul(a, 14) ^ gf_mul(b, 11) ^ gf_mul(c, 13) ^ gf_mul(d, 9),
            gf_mul(a, 9) ^ gf_mul(b, 14) ^ gf_mul(c, 11) ^ gf_mul(d, 13),
            gf_mul(a, 13) ^ gf_mul(b, 9) ^ gf_mul(c, 14) ^ gf_mul(d, 11),
            gf_mul(a, 11) ^ gf_mul(b, 13) ^ gf_mul(c, 9) ^ gf_mul(d, 14),
        ];
    }
}
const INVERSE_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn encryption_schedule(key: [u8; 16]) -> [u8; 176] {
        let mut sbox = [0u8; 256];
        for (index, &value) in INVERSE_SBOX.iter().enumerate() {
            sbox[usize::from(value)] = index as u8;
        }
        let mut keys = [0; 176];
        keys[..16].copy_from_slice(&key);
        let mut rcon = 1;
        for position in (16..176).step_by(4) {
            let mut temporary: [u8; 4] = keys[position - 4..position].try_into().unwrap();
            if position % 16 == 0 {
                temporary.rotate_left(1);
                for byte in &mut temporary {
                    *byte = sbox[usize::from(*byte)];
                }
                temporary[0] ^= rcon;
                rcon = gf_mul(rcon, 2);
            }
            for index in 0..4 {
                keys[position + index] = keys[position - 16 + index] ^ temporary[index];
            }
        }
        keys
    }

    #[test]
    fn expanded_schedule_matches_nist_aes128_known_answer() {
        // FIPS-197 Appendix C.1 uses the standard inverse cipher, independent
        // of the baseline's game keys and encrypted-file fixtures.
        let encryption = encryption_schedule(core::array::from_fn(|index| index as u8));
        let mut inverse = [0; 176];
        for round in 0..11 {
            let mut key: [u8; 16] = encryption[(10 - round) * 16..(11 - round) * 16]
                .try_into()
                .unwrap();
            if (1..10).contains(&round) {
                inverse_mix(&mut key);
            }
            inverse[round * 16..round * 16 + 16].copy_from_slice(&key);
        }
        let mut block = [
            0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
            0xc5, 0x5a,
        ];
        decrypt_expanded(&mut block, &inverse);
        assert_eq!(
            block,
            [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff
            ]
        );
    }

    #[test]
    fn truncated_headers_and_tot_post_regions_fail_before_mutation() {
        for length in 0..0x94 {
            let mut data = vec![0; length];
            if length >= 4 {
                data[..4].copy_from_slice(b"mr0k");
            }
            assert!(decrypt(&data, "BH3PrePre").is_err());
        }
        let mut short = vec![0; 0x94];
        short[..4].copy_from_slice(b"mr0k");
        assert_eq!(
            decrypt(&short, "BH3PrePre").unwrap().len(),
            short.len() - 20
        );
        assert!(decrypt(&short, "TOT").is_err());
        assert!(decrypt(&short, "unknown").is_err());
        assert_eq!(&short[..4], b"mr0k");
    }
}

#[cfg(test)]
mod differential {
    use super::*;
    #[test]
    fn original_csharp_mr0k_outputs_match_all_bytes_at_boundary_lengths() {
        for (profile, length, expected) in [
            (
                "BH3PrePre",
                148,
                include_bytes!("../tests/fixtures/mhy-oracle/BH3PrePre-148.bin").as_slice(),
            ),
            (
                "BH3PrePre",
                276,
                include_bytes!("../tests/fixtures/mhy-oracle/BH3PrePre-276.bin").as_slice(),
            ),
            (
                "BH3PrePre",
                4096,
                include_bytes!("../tests/fixtures/mhy-oracle/BH3PrePre-4096.bin").as_slice(),
            ),
            (
                "BH3PrePre",
                10000,
                include_bytes!("../tests/fixtures/mhy-oracle/BH3PrePre-10000.bin").as_slice(),
            ),
            (
                "TOT",
                3092,
                include_bytes!("../tests/fixtures/mhy-oracle/TOT-3092.bin").as_slice(),
            ),
            (
                "TOT",
                4096,
                include_bytes!("../tests/fixtures/mhy-oracle/TOT-4096.bin").as_slice(),
            ),
            (
                "TOT",
                10000,
                include_bytes!("../tests/fixtures/mhy-oracle/TOT-10000.bin").as_slice(),
            ),
        ] {
            let mut input: Vec<u8> = (0..length)
                .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
                .collect();
            input[..4].copy_from_slice(b"mr0k");
            assert_eq!(
                decrypt(&input, profile).unwrap(),
                expected,
                "{profile} length {length}"
            );
        }
    }
}
