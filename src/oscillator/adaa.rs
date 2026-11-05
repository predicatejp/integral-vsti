// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! Periodic primitive tables and cancellation-safe divided differences for phase modulation.
//! All phases in a difference use the same current band and unwrapped cycle coordinates.

#[derive(Clone, Copy)]
struct Table {
    /// Intervals per cycle: 4096 stores a full cycle, larger tables a half cycle.
    n: f64,
    /// Highest retained harmonic.
    h: f64,
    /// Second-primitive coefficient offset (f64 elements).
    offset: usize,
    /// Twice the squared cell count, used by the second derivative.
    scale: f64,
}
#[path = "adaa_data.rs"]
mod tables;
static SECOND: &[u8] = include_bytes!("f2.bin");
#[derive(Clone, Copy)]
pub(super) struct Bank {
    /// Richer band selected from the unmodulated frequency.
    hi: &'static Table,
    /// Adjacent band used while removing the highest harmonics.
    lo: &'static Table,
    /// Crossfade toward the adjacent band.
    mix: f64,
    /// Fundamental fade immediately below Nyquist.
    gain: f64,
}
impl Bank {
    pub fn new(w: super::Waveform, inc: f64) -> Self {
        let ts: &'static [Table] = match w {
            super::Waveform::Sine => &tables::SINE,
            super::Waveform::Triangle => &tables::TRIANGLE,
            super::Waveform::Square => &tables::SQUARE,
            _ => &tables::SAW,
        };
        let idx = if w == super::Waveform::Sine {
            0
        } else {
            ts.partition_point(|t| t.h * inc < 0.5).saturating_sub(1)
        };
        let mix = if w == super::Waveform::Sine {
            0.
        } else {
            ((inc * ts[idx].h * 2. - 0.9) * 10.).clamp(0., 1.)
        };
        Self {
            hi: &ts[idx],
            lo: &ts[idx.saturating_sub(1)],
            mix: if idx == 0 { 0. } else { mix },
            gain: if idx == 0 && w != super::Waveform::Sine {
                1. - mix
            } else {
                1.
            },
        }
    }
    #[cfg(test)]
    pub fn read_sample(self, q: f64) -> f64 {
        self.blend(|t| {
            if t.n == 4096. {
                t.raw::<false>(q)
            } else {
                t.raw::<true>(q)
            }
        })
    }
    fn blend(self, f: impl Fn(&Table) -> f64) -> f64 {
        let v = f(self.hi);
        self.gain
            * if self.mix == 0. {
                v
            } else {
                v + (f(self.lo) - v) * self.mix
            }
    }
    pub fn generate_sample(self, a: f64, b: f64, c: f64) -> f64 {
        let (mut x, mut y, mut z) = (a, b, c);
        if x > y {
            std::mem::swap(&mut x, &mut y)
        }
        if y > z {
            std::mem::swap(&mut y, &mut z)
        }
        if x > y {
            std::mem::swap(&mut x, &mut y)
        }
        self.blend(|t| {
            if t.n == 4096. {
                t.second::<false>(x, y, z)
            } else {
                t.second::<true>(x, y, z)
            }
        })
    }
}
impl Table {
    #[inline]
    fn loc<const FOLDED: bool>(self, q: f64) -> (usize, f64, f64) {
        // Large tables store the positive half cycle: F2 is odd and F1 is even.
        // Small tables retain a full cycle to avoid reflection in the common path.
        let phase = q - (q + if FOLDED { 0.5 } else { 0.0 }).floor();
        let x = if FOLDED { phase.abs() } else { phase } * self.n;
        // SAFETY: Oscillator supplies finite, bounded modulated phases. Folding
        // gives |phase| <= 1/2 for large tables (n <= 131072), or phase <= 1
        // for full-cycle tables (n = 4096). Thus x is in [0,65536].
        // Each table includes its endpoint cell.
        let i = unsafe { x.to_int_unchecked::<i32>() };
        (
            i as usize,
            x - f64::from(i),
            if FOLDED { 1.0_f64.copysign(phase) } else { 1.0 },
        )
    }
    #[inline]
    fn coefficients(self, i: usize) -> [f64; 6] {
        read_coefficients(SECOND, self.offset + 6 * i)
    }
    fn p1<const FOLDED: bool>(self, q: f64) -> f64 {
        let (i, t, _) = self.loc::<FOLDED>(q);
        let c = self.coefficients(i);
        self.n * (c[1] + t * (2. * c[2] + t * (3. * c[3] + t * (4. * c[4] + t * 5. * c[5]))))
    }
    fn p2<const FOLDED: bool>(self, q: f64) -> f64 {
        let (i, t, sign) = self.loc::<FOLDED>(q);
        let c = self.coefficients(i);
        let value = c[0] + t * (c[1] + t * (c[2] + t * (c[3] + t * (c[4] + t * c[5]))));
        // Odd reflection changes only the sign bit, avoiding multiplication by +/-1.
        f64::from_bits(value.to_bits() ^ (sign.to_bits() & (1 << 63)))
    }
    fn raw<const FOLDED: bool>(self, q: f64) -> f64 {
        let (i, t, sign) = self.loc::<FOLDED>(q);
        let c = self.coefficients(i);
        sign * self.scale * (c[2] + t * (3. * c[3] + t * (6. * c[4] + t * 10. * c[5])))
    }
    // Keep the ordinary subtraction/division in the per-sample path inline.
    #[inline(always)]
    fn sec<const FOLDED: bool>(self, a: f64, b: f64, ga: f64, gb: f64) -> f64 {
        let d = b - a;
        if d * self.n < 0.0001 {
            self.p1::<FOLDED>((a + b) * 0.5)
        } else {
            (gb - ga) / d
        }
    }
    fn second<const FOLDED: bool>(self, a: f64, b: f64, c: f64) -> f64 {
        let span = c - a;
        if span * self.n < 1. {
            let (i, x, sign) = self.loc::<FOLDED>(a);
            let direction = sign;
            let y = x + (b - a) * self.n * direction;
            let z = x + span * self.n * direction;
            if z <= 1.0 && (!FOLDED || z >= 0.0) {
                let p = self.coefficients(i);
                let e1 = x + y + z;
                let e2 = x * y + x * z + y * z;
                let h2 = e1 * e1 - e2;
                let h3 = e1 * h2 - e2 * e1 + x * y * z;
                return sign * self.scale * (p[2] + p[3] * e1 + p[4] * h2 + p[5] * h3);
            }
            // Across a cell boundary, integrate the raw cubic over the triangle.
            // The seven-point degree-5 rule avoids subtracting nearly equal primitives.
            let mut y = 0.225 * self.raw::<FOLDED>((a + b + c) / 3.);
            for (t, w) in [
                (0.059715871789770, 0.132394152788506),
                (0.797426985353087, 0.125939180544827),
            ] {
                let s = (1. - t) * 0.5;
                y += w
                    * (self.raw::<FOLDED>(t * a + s * (b + c))
                        + self.raw::<FOLDED>(t * b + s * (a + c))
                        + self.raw::<FOLDED>(t * c + s * (a + b)));
            }
            return y;
        }
        let ga = self.p2::<FOLDED>(a);
        let gb = self.p2::<FOLDED>(b);
        let gc = self.p2::<FOLDED>(c);
        2. * (self.sec::<FOLDED>(b, c, gb, gc) - self.sec::<FOLDED>(a, b, ga, gb)) / span
    }
}

#[inline]
fn read_coefficients<const N: usize>(data: &[u8], offset: usize) -> [f64; N] {
    std::array::from_fn(|i| {
        let at = (offset + i) * 8;
        f64::from_le_bytes(data[at..at + 8].try_into().unwrap())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oscillator::Waveform;
    use std::f64::consts::TAU;

    #[test]
    fn sine_divided_differences_match_analytic_primitives() {
        let bank = Bank::new(Waveform::Sine, 0.02);
        for a in [-2.0, -0.271, 0.0, 0.125, 0.999999, 2.2] {
            for delta in [0.0_f64, 1e-12, 1e-8, 0.0002, 0.01, 0.2, 1.4] {
                let b = a + delta;
                let c = a + delta * 2.3;
                let expected = crate::oscillator::reference::antialiased_sample(
                    Waveform::Sine,
                    [a, b, c],
                    0.02,
                );
                for q in [
                    [a, b, c],
                    [c, a, b],
                    [b, c, a],
                    [a, c, b],
                    [b, a, c],
                    [c, b, a],
                ] {
                    assert!((bank.generate_sample(q[0], q[1], q[2]) - expected).abs() < 1e-8);
                }
            }
        }
    }

    #[test]
    fn primitive_derivatives_and_repeated_points_preserve_band_waveforms() {
        for wave in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
        ] {
            for inc in [0.00002, 0.002, 0.02, 0.12, 0.49, 0.5] {
                let bank = Bank::new(wave, inc);
                for i in 0..193 {
                    let phase = (i as f64 + 0.31) / 193.0;
                    let expected = if wave == Waveform::Sine {
                        (TAU * phase).sin()
                    } else {
                        crate::oscillator::wavetable::Selection::new(wave, inc)
                            .generate_sample(wave, phase)
                    };
                    assert!((bank.read_sample(phase) - expected).abs() < 1e-6);
                    assert!((bank.generate_sample(phase, phase, phase) - expected).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn folded_lookup_covers_valid_phase_endpoints() {
        for wave in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
        ] {
            for inc in [0.000001, 0.002, 0.49] {
                let bank = Bank::new(wave, inc);
                // Input modulation fits i32; phase/history can extend it by two cycles.
                for origin in [
                    f64::from(i32::MIN) - 2.0,
                    -1.0,
                    0.0,
                    f64::from(i32::MAX) + 1.0,
                ] {
                    for offset in [0.0, 0.5_f64.next_down(), 0.5, 0.5_f64.next_up(), 1.0] {
                        let phase = origin + offset;
                        let expected = bank.read_sample(phase - phase.floor());
                        let actual = bank.generate_sample(phase, phase, phase);
                        assert!(actual.is_finite() && (actual - expected).abs() < 1e-6);
                    }
                }
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 密な数値積分との照合は通常のCPUテストで行う。
    fn close_points_across_symmetry_folds_preserve_divided_differences() {
        // Integrate the original waveform with a triangular weight, independently
        // of primitive subtraction. Tiny jumps in F2 otherwise grow as 1/span².
        fn integral(raw: &impl Fn(f64) -> f64, a: f64, b: f64, rising: bool) -> f64 {
            const STEPS: usize = 64;
            let sum: f64 = (0..=STEPS)
                .map(|i| {
                    let t = i as f64 / STEPS as f64;
                    let weight = if rising { t } else { 1.0 - t };
                    let simpson = if i == 0 || i == STEPS {
                        1.0
                    } else if i % 2 == 0 {
                        2.0
                    } else {
                        4.0
                    };
                    simpson * weight * raw(a + (b - a) * t)
                })
                .sum();
            (b - a) * sum / (3 * STEPS) as f64
        }
        for wave in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
        ] {
            for harmonic in [1.0, 64.0, 16384.0] {
                let inc = 0.1 / harmonic;
                let bank = Bank::new(wave, inc);
                let selection = (wave != Waveform::Sine)
                    .then(|| crate::oscillator::wavetable::Selection::new(wave, inc));
                let raw = |q: f64| {
                    if wave == Waveform::Sine {
                        (TAU * q).sin()
                    } else {
                        selection.unwrap().generate_sample(wave, q - q.floor())
                    }
                };
                let mut signal = 0.0;
                let mut error = 0.0;
                for fold in [-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0] {
                    for width in [0.6, 1.5] {
                        let span = width / bank.hi.n;
                        let a = fold - span * 0.03;
                        let c = a + span;
                        for fraction in [0.0, 1e-8, 0.3, 1.0] {
                            let b = a + span * fraction;
                            let expected = 2.0
                                * (integral(&raw, a, b, true) + integral(&raw, b, c, false))
                                / span;
                            let actual = bank.generate_sample(a, b, c);
                            signal += expected * expected;
                            error += (actual - expected).powi(2);
                        }
                    }
                }
                assert!(
                    error <= signal * 1e-8,
                    "{wave:?}, {harmonic}: {error}/{signal}"
                );
            }
        }
    }
}
