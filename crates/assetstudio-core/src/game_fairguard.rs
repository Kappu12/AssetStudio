//! FairGuard block compatibility transcribed from the baseline managed reader.
//!
//! `AssetStudio/Crypto/FairGuardUtils.cs` credits LukeFZ. The repository MIT
//! notice applies. This is a reader for an existing format, not a new cipher.

const MAX_ENCRYPTED_BYTES: usize = 0x500;
const HEADER_BYTES: usize = 0x20;

/// Only the first 1280 bytes are transformed. Shorter-than-header blocks are
/// intentionally unchanged, matching the original reader's early return.
pub(super) fn decrypt(input: &mut [u8]) {
    let encrypted_size = input.len().min(MAX_ENCRYPTED_BYTES);
    if encrypted_size < HEADER_BYTES {
        return;
    }
    let encrypted = &mut input[..encrypted_size];
    for byte in &mut encrypted[..HEADER_BYTES] {
        *byte ^= 0xa6;
    }
    let word = |index: usize| {
        u32::from_le_bytes(
            encrypted[index * 4..index * 4 + 4]
                .try_into()
                .expect("eight validated header words"),
        )
    };
    let size = encrypted_size as u32;
    let mut seeds = [
        word(2) ^ 0x1274_cbec ^ word(6) ^ 0x3f72_eaf3,
        word(3) ^ 0xbe48_2704 ^ word(0) ^ size,
        word(1) ^ size ^ word(5) ^ 0x753b_dcaa,
        word(0) ^ 0x82c5_7e3c ^ word(7) ^ 0xe3d9_47d3,
        word(4) ^ 0x6f2a_7347 ^ word(7) ^ 0x4736_c714,
    ];
    let mut seed_bytes = [0u8; 20];
    for (target, source) in seed_bytes.as_chunks_mut::<4>().0.iter_mut().zip(seeds) {
        target.copy_from_slice(&source.to_le_bytes());
    }
    let seed = crc(&generate_seed(&seed_bytes).to_le_bytes());
    let key = seeds.iter().fold(size, |value, part| value ^ part);
    rc4(&mut seed_bytes, key);
    let key_seed = generate_seed(&crc(&seed_bytes).to_le_bytes());
    // MemoryMarshal.AsBytes aliases the C# seed array. The later key words
    // therefore use the RC4-mutated bytes, not the original five seed words.
    for (target, source) in seeds.iter_mut().zip(seed_bytes.as_chunks::<4>().0) {
        *target = u32::from_le_bytes(*source);
    }
    let key_vector = [
        seeds[3].wrapping_sub(0x1c26_b82d) ^ key_seed,
        seeds[2].wrapping_add(0x3f72_eaf3) ^ seed,
        seeds[0] ^ 0x82c5_7e3c ^ key_seed,
        seeds[1].wrapping_add(0x6f2a_7347) ^ seed,
    ];
    let block = &mut encrypted[HEADER_BYTES..];
    if block.len() < 0x80 {
        rc4(block, seed);
        return;
    }
    rc4(&mut block[..0x60], seed);
    for byte in &mut block[..0x60] {
        *byte ^= (seed as u8) ^ 0x6e;
    }
    let block = &mut block[0x60..];
    let segment_size = (encrypted_size - 0x80) / 4;
    for (index, block_key) in [0x6142_756e, 0x6249_6e66, 0x1304_b000, 0x6e8e_30ec]
        .into_iter()
        .enumerate()
    {
        let start = index * segment_size;
        let segment = &mut block[start..start + segment_size];
        rc4(segment, seed);
        let mask = seed ^ key_vector[index] ^ block_key;
        for bytes in segment.as_chunks_mut::<4>().0 {
            let word = u32::from_le_bytes(*bytes);
            bytes.copy_from_slice(&(word ^ mask).to_le_bytes());
        }
    }
}

fn generate_seed(bytes: &[u8]) -> u32 {
    let mut state = [0xc164_6153u32, 0x78da_0550, 0x2947_e56b];
    for byte in bytes {
        state[0] = state[0].wrapping_mul(0x21).wrapping_add(u32::from(*byte));
        if state[0] & 0xf >= 0xb {
            state[0] = (state[0] ^ rotate_is_set(state[2], 6)).wrapping_sub(0x2cd8_6315);
        } else if (state[0] & 0xf0) >> 4 > 0xe {
            state[0] = (state[1] ^ 0xab4a_010b).wrapping_add(state[0] ^ rotate_is_set(state[2], 9));
        } else if (state[0] & 0xf00) >> 8 < 2 {
            state[1] = (state[2] >> 3).wrapping_sub(0x55ee_ab7b) ^ state[0];
        } else if state[1].wrapping_add(0x567a) >= 0xab54_89e4 {
            state[1] = (state[1] >> 16) ^ state[0];
        } else if state[1] ^ 0x7387_66fa <= state[2] {
            state[1] = (state[1] >> 8) ^ state[2];
        } else if state[1] == 0x68f5_3aa6 {
            if state[1] ^ state[0].wrapping_add(state[2]) > 0x594a_f86e {
                state[1] = state[1].wrapping_sub(0x08ca_292e);
            } else {
                state[2] = state[2].wrapping_sub(0x760a_1649);
            }
        } else {
            state[1] = if state[0] > 0x8657_03af {
                state[2] ^ state[0].wrapping_sub(0x5643_89d7)
            } else {
                state[1].wrapping_sub(0x12b9_dd92) ^ state[0]
            };
            state[0] ^= rotate_is_set(state[1], 8);
        }
    }
    state[0]
}

fn rotate_is_set(value: u32, count: u32) -> u32 {
    // The original tests whether either shifted half is nonzero; it does not
    // return the rotated integer. Preserve its boolean-to-integer result.
    u32::from(value >> count != 0 || value << (32 - count) != 0)
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < table.len() {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ if value & 1 != 0 { 0xd35e_417e } else { 0 };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const CRC_TABLE: [u32; 256] = crc_table();

fn crc(bytes: &[u8]) -> u32 {
    let mut value = u32::MAX;
    for byte in bytes {
        value = (CRC_TABLE[usize::from(value as u8 ^ byte)] ^ (value >> 9)).wrapping_add(0x5b);
    }
    (!value).wrapping_sub(0x4160_7a3d)
}

fn rc4(bytes: &mut [u8], key: u32) {
    let key = key.to_le_bytes();
    let mut state: [u8; 256] = std::array::from_fn(|index| index as u8);
    let mut j = 0u8;
    for i in 0..state.len() {
        j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
        state.swap(i, usize::from(j));
    }
    let mut i = 0u8;
    j = 0;
    for byte in bytes {
        i = i.wrapping_add(1);
        j = j.wrapping_add(state[usize::from(i)]);
        state.swap(usize::from(i), usize::from(j));
        let slot = state[usize::from(i)].wrapping_add(state[usize::from(j)]);
        *byte ^= state[usize::from(slot)].rotate_left(1).wrapping_sub(0x61);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_csharp_fairguard_matches_every_byte_at_all_branch_boundaries() {
        macro_rules! golden {
            ($length:literal) => {
                (
                    $length,
                    include_bytes!(concat!(
                        "../tests/fixtures/endfield-oracle/fairguard-",
                        $length,
                        ".bin"
                    ))
                    .as_slice(),
                )
            };
        }
        for (length, expected) in [
            golden!(0),
            golden!(1),
            golden!(31),
            golden!(32),
            golden!(33),
            golden!(80),
            golden!(159),
            golden!(160),
            golden!(161),
            golden!(255),
            golden!(1279),
            golden!(1280),
            golden!(1281),
            golden!(4096),
        ] {
            let mut input: Vec<u8> = (0..length)
                .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
                .collect();
            let original = input.clone();
            decrypt(&mut input);
            assert_eq!(input, expected, "FairGuard input length {length}");
            if length > MAX_ENCRYPTED_BYTES {
                assert_eq!(
                    input[MAX_ENCRYPTED_BYTES..],
                    original[MAX_ENCRYPTED_BYTES..]
                );
            }
        }
    }
}
