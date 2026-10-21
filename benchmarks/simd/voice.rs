// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
// 実験コピーのvoice.rsへ追加。オシレータ/エンベロープ/ノート割当は既存実装を使用する。
impl Voice {
    pub(crate) fn generate_parallel_sample(
        voices: &mut [Self],
        rate: f64,
    ) -> [f64; crate::simd::LANES] {
        use crate::simd::LANES;
        let kind = voices[0].state.filter_type; // Synthesizerの共通パラメーター。
        let samples: [f64; LANES] = std::array::from_fn(|i| {
            voices[i]
                .state
                .oscillator
                .generate_sample(&voices[i].state.oscillator_input, rate)
        });
        let parameters: [FilterParameters; LANES] = std::array::from_fn(|i| FilterParameters {
            cutoff_hz: (voices[i].state.oscillator_input.frequency_hz
                * voices[i].state.cutoff_ratio)
                .min(f64::MAX),
            resonance_q: voices[i].state.resonance_q,
        });
        let mut iter = voices.iter_mut();
        let mut filters: [&mut Filter; LANES] =
            std::array::from_fn(|_| &mut iter.next().unwrap().state.filter);
        Filter::process_parallel_sample(&mut filters, &samples, kind, &parameters, rate)
    }
}
