use std::hash::{BuildHasherDefault, Hasher};

/// FxHash（rustc-hash と同アルゴリズム）の軽量実装。
///
/// ソルバー内部の HashMap/HashSet と各種ハッシュ計算は、キーが既に
/// Zobrist ハッシュ由来の u64 か小さな構造体であり、DoS 耐性は不要。
/// 標準の SipHash はプロファイルで自己時間の 15-22% を占めていたため、
/// 乗算ベースの高速ハッシュに置き換える。
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add_to_hash(&mut self, w: u64) {
        self.hash = (self.hash.rotate_left(5) ^ w).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        while bytes.len() >= 8 {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes[..8]);
            self.add_to_hash(u64::from_le_bytes(buf));
            bytes = &bytes[8..];
        }
        if bytes.len() >= 4 {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&bytes[..4]);
            self.add_to_hash(u32::from_le_bytes(buf) as u64);
            bytes = &bytes[4..];
        }
        for &b in bytes {
            self.add_to_hash(b as u64);
        }
    }

    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.add_to_hash(n as u64);
    }

    #[inline]
    fn write_u16(&mut self, n: u16) {
        self.add_to_hash(n as u64);
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add_to_hash(n as u64);
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add_to_hash(n);
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add_to_hash(n as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type FxBuildHasher = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;
pub type FxHashSet<T> = std::collections::HashSet<T, FxBuildHasher>;
