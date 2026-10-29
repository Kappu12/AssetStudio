//! Small compatibility algorithms used by game wrapper decoding.
//!
//! RC4 and the historical .NET subtractive PRNG are format compatibility,
//! never encryption choices for newly produced data.

use aes::Aes256;
use aes::cipher::{Block, BlockEncrypt, KeyInit};
use anyhow::{Result, ensure};
use md5::{Digest, Md5};

pub(super) fn rc4(bytes: &mut [u8], key: &[u8]) -> Result<()> {
    ensure!(!key.is_empty(), "RC4 compatibility key is empty");
    let mut state = [0_u8; 256];
    for (index, slot) in state.iter_mut().enumerate() {
        *slot = index as u8;
    }
    let mut j = 0_u8;
    for i in 0..256 {
        j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
        state.swap(i, usize::from(j));
    }
    let (mut i, mut j) = (0_u8, 0_u8);
    for byte in bytes {
        i = i.wrapping_add(1);
        j = j.wrapping_add(state[usize::from(i)]);
        state.swap(usize::from(i), usize::from(j));
        let k = state[usize::from(state[usize::from(i)].wrapping_add(state[usize::from(j)]))];
        *byte ^= k;
    }
    Ok(())
}

pub(super) fn md5_hex(value: &str) -> String {
    let digest = Md5::digest(value.as_bytes());
    let mut output = String::with_capacity(32);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 15)] as char);
    }
    output
}

/// Uses the exact compatibility constructor behind .NET `new Random(int)`.
/// Seeded .NET `Random` preserves this algorithm across recent .NET versions.
pub(super) struct DotNetRandom {
    seed_array: [i32; 56],
    next: usize,
    next_prime: usize,
}

impl DotNetRandom {
    pub(super) fn new(seed: i32) -> Self {
        const BIG: i32 = i32::MAX;
        let subtract = seed.checked_abs().unwrap_or(BIG);
        let mut mj = 161_803_398_i32 - subtract;
        let mut seed_array = [0; 56];
        seed_array[55] = mj;
        let mut mk = 1;
        for i in 1..55 {
            let ii = 21 * i % 55;
            seed_array[ii] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk += BIG;
            }
            mj = seed_array[ii];
        }
        for _ in 0..4 {
            for i in 1..56 {
                seed_array[i] = seed_array[i].wrapping_sub(seed_array[1 + (i + 30) % 55]);
                if seed_array[i] < 0 {
                    seed_array[i] += BIG;
                }
            }
        }
        Self {
            seed_array,
            next: 0,
            next_prime: 21,
        }
    }

    pub(super) fn sample(&mut self) -> i32 {
        self.next = if self.next + 1 >= 56 {
            1
        } else {
            self.next + 1
        };
        self.next_prime = if self.next_prime + 1 >= 56 {
            1
        } else {
            self.next_prime + 1
        };
        let mut value = self.seed_array[self.next].wrapping_sub(self.seed_array[self.next_prime]);
        if value == i32::MAX {
            value -= 1;
        }
        if value < 0 {
            value += i32::MAX;
        }
        self.seed_array[self.next] = value;
        value
    }

    pub(super) fn range(&mut self, minimum: usize, maximum: usize) -> usize {
        minimum
            + ((f64::from(self.sample()) / f64::from(i32::MAX)) * (maximum - minimum) as f64)
                as usize
    }
}

pub(super) fn jjk_counter_xor(bytes: &mut [u8]) -> Result<()> {
    let cipher = Aes256::new_from_slice(b"6154e00f9E9ce46dc9054E07173Aa546")?;
    for (index, chunk) in bytes.chunks_mut(16).enumerate() {
        let index = u64::try_from(index)?;
        let counter = (index / 64 + 1)
            .checked_mul(100)
            .and_then(|value| value.checked_add(index % 64))
            .ok_or_else(|| anyhow::anyhow!("JJK counter overflow"))?;
        let mut block = Block::<Aes256>::default();
        block[..8].copy_from_slice(&counter.to_le_bytes());
        cipher.encrypt_block(&mut block);
        for (target, key) in chunk.iter_mut().zip(block) {
            *target ^= key;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotnet_seed_one_matches_documented_sequence() {
        let mut random = DotNetRandom::new(1);
        let actual: Vec<i32> = (0..5).map(|_| random.sample()).collect();
        assert_eq!(
            actual,
            [
                534_011_718,
                237_820_880,
                1_002_897_798,
                1_657_007_234,
                1_412_011_072
            ]
        );
    }

    #[test]
    fn dotnet_seed_min_value_does_not_overflow() {
        let mut random = DotNetRandom::new(i32::MIN);
        for _ in 0..1000 {
            assert!((0..i32::MAX).contains(&random.sample()));
        }
    }

    #[test]
    fn md5_reference_digest() {
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn rc4_reference_vector() {
        let mut bytes = b"Plaintext".to_vec();
        rc4(&mut bytes, b"Key").unwrap();
        assert_eq!(
            bytes,
            [0xbb, 0xf3, 0x16, 0xe8, 0xd9, 0x40, 0xaf, 0x0a, 0xd3]
        );
    }
}
