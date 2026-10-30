// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use std::{collections::hash_map::RandomState as SeedSource, hash::BuildHasher};

/// xoshiro256+の内部状態。全要素0の状態は使用しない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RandomState {
    /// 256bitの状態語。
    words: [u64; 4],
}

/// 位相とノイズで共用する乱数生成器。各用途は独立したインスタンスを所有する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Random {
    /// この生成器の内部状態。
    state: RandomState,
}

impl Random {
    /// シード取得は初期化時だけ行い、音声処理から呼ばない。
    pub fn new() -> Self {
        let source = SeedSource::new();
        Self::from_seed(std::array::from_fn(|i| source.hash_one(i)))
    }

    /// 指定した状態語から開始する。全要素0は停止状態を避けるため補正する。
    pub fn from_seed(mut words: [u64; 4]) -> Self {
        if words == [0; 4] {
            words[0] = 1;
        }
        Self {
            state: RandomState { words },
        }
    }

    /// xoshiro256+ 1.0の上位53bitを[0,1)へ換算し、次の乱数へ進める。
    /// 参照元: https://prng.di.unimi.it/xoshiro256plus.c （通知はTHIRD_PARTY_NOTICES.txt）。
    pub fn next_f64(&mut self) -> f64 {
        let s = &mut self.state.words;
        let bits = s[0].wrapping_add(s[3]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        (bits >> 11) as f64 * (1.0 / (1_u64 << 53) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_sequence_and_replays_from_state() {
        let mut random = Random::from_seed([
            0x1234_5678_9abc_def0,
            0xfedc_ba98_7654_3210,
            0x1020_3040_5060_7080,
            0x8877_6655_4433_2211,
        ]);
        // 作者のC実装を同じ初期状態で実行した上位53bit。
        for expected in [
            5441996726197760_u64,
            2484858452741806,
            5686259996541749,
            7701203122316008,
            8048927005211174,
            5195266920391698,
            2168565655257186,
            1019535850546686,
        ] {
            assert_eq!(random.next_f64(), expected as f64 / (1_u64 << 53) as f64);
        }
        let state = random.state;
        let expected: [f64; 7] = std::array::from_fn(|_| random.next_f64());
        random.state = state;
        assert_eq!(
            std::array::from_fn::<_, 7, _>(|_| random.next_f64()),
            expected
        );
    }

    #[test]
    fn zero_seed_does_not_lock_generator_and_output_stays_in_range() {
        let mut random = Random::from_seed([0; 4]);
        let mut nonzero = false;
        for _ in 0..1024 {
            let value = random.next_f64();
            assert!((0.0..1.0).contains(&value));
            nonzero |= value != 0.0;
        }
        assert!(nonzero);
    }
}
