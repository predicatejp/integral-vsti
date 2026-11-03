// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! Periodic primitive tables and cancellation-safe divided differences for phase modulation.
//! All phases in a difference use the same current band and unwrapped cycle coordinates.

#[derive(Clone, Copy)]
struct Table {
    /// Number of polynomial intervals per cycle.
    n: f64,
    /// Highest retained harmonic.
    h: f64,
    /// First primitive coefficient offset (f64 elements).
    a: usize,
    /// Second primitive coefficient offset (f64 elements).
    b: usize,
}
#[path = "adaa_data.rs"]
mod tables;
static FIRST: &[u8] = include_bytes!("f1.bin");
static SECOND: &[u8] = include_bytes!("f2.bin");
#[derive(Clone, Copy)]
pub(super) struct Bank {
    /// Richer band selected from the unmodulated frequency.
    hi: Table,
    /// Adjacent band used while removing the highest harmonics.
    lo: Table,
    /// Crossfade toward the adjacent band.
    mix: f64,
    /// Fundamental fade immediately below Nyquist.
    gain: f64,
}
impl Bank {
    pub fn new(w: super::Waveform, inc: f64) -> Self {
        let ts: &[Table] = match w {
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
            hi: ts[idx],
            lo: ts[idx.saturating_sub(1)],
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
        self.blend(|t| t.raw(q, FIRST))
    }
    fn blend(self, f: impl Fn(Table) -> f64) -> f64 {
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
        self.blend(|t| t.second(x, y, z, FIRST, SECOND))
    }
}
impl Table {
    #[inline]
    fn loc(self, q: f64) -> (usize, f64) {
        let x = (q - q.floor()) * self.n;
        let i = (x as usize).min(self.n as usize - 1);
        (i, x - i as f64)
    }
    #[inline]
    fn c1(self, i: usize, data: &[u8]) -> [f64; 5] {
        read_coefficients(data, self.a + 5 * i)
    }
    #[inline]
    fn c2(self, i: usize, data: &[u8]) -> [f64; 6] {
        read_coefficients(data, self.b + 6 * i)
    }
    fn p1(self, q: f64, data: &[u8]) -> f64 {
        let (i, t) = self.loc(q);
        let c = self.c1(i, data);
        c[0] + t * (c[1] + t * (c[2] + t * (c[3] + t * c[4])))
    }
    fn p2(self, q: f64, data: &[u8]) -> f64 {
        let (i, t) = self.loc(q);
        let c = self.c2(i, data);
        c[0] + t * (c[1] + t * (c[2] + t * (c[3] + t * (c[4] + t * c[5]))))
    }
    fn raw(self, q: f64, data: &[u8]) -> f64 {
        let (i, t) = self.loc(q);
        let c = self.c1(i, data);
        self.n * (c[1] + t * (2. * c[2] + t * (3. * c[3] + t * 4. * c[4])))
    }
    fn sec(self, a: f64, b: f64, ga: f64, gb: f64, f1: &[u8]) -> f64 {
        let d = b - a;
        if d * self.n < 0.0001 {
            self.p1((a + b) * 0.5, f1)
        } else {
            (gb - ga) / d
        }
    }
    fn second(self, a: f64, b: f64, c: f64, f1: &[u8], f2: &[u8]) -> f64 {
        let span = c - a;
        if span * self.n < 1. {
            let (i, x) = self.loc(a);
            let y = x + (b - a) * self.n;
            let z = x + span * self.n;
            if z <= 1. {
                let p = self.c2(i, f2);
                let e1 = x + y + z;
                let e2 = x * y + x * z + y * z;
                let h2 = e1 * e1 - e2;
                let h3 = e1 * h2 - e2 * e1 + x * y * z;
                return 2. * self.n * self.n * (p[2] + p[3] * e1 + p[4] * h2 + p[5] * h3);
            }
            // Across a cell boundary, integrate the raw cubic over the triangle.
            // The seven-point degree-5 rule avoids subtracting nearly equal primitives.
            let mut y = 0.225 * self.raw((a + b + c) / 3., f1);
            for (t, w) in [
                (0.059715871789770, 0.132394152788506),
                (0.797426985353087, 0.125939180544827),
            ] {
                let s = (1. - t) * 0.5;
                y += w
                    * (self.raw(t * a + s * (b + c), f1)
                        + self.raw(t * b + s * (a + c), f1)
                        + self.raw(t * c + s * (a + b), f1));
            }
            return y;
        }
        let ga = self.p2(a, f2);
        let gb = self.p2(b, f2);
        let gc = self.p2(c, f2);
        2. * (self.sec(b, c, gb, gc, f1) - self.sec(a, b, ga, gb, f1)) / span
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
}
