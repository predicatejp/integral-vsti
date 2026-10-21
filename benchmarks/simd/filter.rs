// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
// 実験コピーのfilter.rsへ追加。各Voiceの既存状態を集め、同じ時刻のSVFを並列処理する。
impl Filter {
    pub(crate) fn process_parallel_sample(
        filters: &mut [&mut Self],
        input: &[f64],
        kind: FilterType,
        parameters: &[FilterParameters],
        rate: f64,
    ) -> [f64; crate::simd::LANES] {
        use crate::simd::{LANES, SvfLanes};
        let mut samples = [0.0; LANES];
        samples.copy_from_slice(input);
        if matches!(kind, FilterType::None) {
            for filter in filters {
                filter.reset();
            }
            return samples;
        }
        let count = if matches!(
            kind,
            FilterType::LowPass2 | FilterType::BandPass2 | FilterType::HighPass2
        ) {
            2
        } else {
            1
        };
        let output = match kind {
            FilterType::LowPass1 | FilterType::LowPass2 => 0,
            FilterType::BandPass1 | FilterType::BandPass2 => 1,
            _ => 2,
        };
        let mut enabled = [false; LANES];
        let mut lanes = SvfLanes {
            x: [0.0; LANES],
            state: crate::simd::SvfLaneState {
                s1: [0.0; LANES],
                s2: [0.0; LANES],
            },
            g: [0.0; LANES],
            damping_plus_g: [0.0; LANES],
            denominator: [1.0; LANES],
        };
        for i in 0..LANES {
            let filter = &mut filters[i];
            if !valid_input(input[i], &parameters[i], rate) {
                samples[i] = 0.0;
                continue;
            }
            if count == 1 {
                filter.state.stages[1].state.integrators = [0.0; 2];
            }
            if input[i] == 0.0
                && filter.state.stages[..count]
                    .iter()
                    .all(|s| s.state.integrators == [0.0; 2])
            {
                continue;
            }
            enabled[i] = true;
            let c = filter.state.stages[0].prepare_coefficients(&parameters[i], rate);
            lanes.g[i] = c.g;
            lanes.damping_plus_g[i] = c.damping_plus_g;
            lanes.denominator[i] = c.denominator;
        }
        if !enabled.iter().any(|&v| v) {
            return samples;
        }
        for stage in 0..count {
            for i in 0..LANES {
                lanes.x[i] = if enabled[i] { samples[i] } else { 0.0 };
                let state = if enabled[i] {
                    filters[i].state.stages[stage].state.integrators
                } else {
                    [0.0; 2]
                };
                lanes.state.s1[i] = state[0];
                lanes.state.s2[i] = state[1];
            }
            let (output_samples, finite) = lanes.process_sample(output);
            samples = output_samples;
            for i in 0..LANES {
                if enabled[i] {
                    if finite[i] {
                        filters[i].state.stages[stage].state.integrators =
                            [lanes.state.s1[i], lanes.state.s2[i]];
                    } else {
                        filters[i].state.stages[stage].state.integrators = [0.0; 2];
                        samples[i] = 0.0;
                    }
                }
            }
        }
        samples
    }
}
