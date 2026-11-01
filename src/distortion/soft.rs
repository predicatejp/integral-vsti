// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! Soft用の整合した原始関数と第2原始関数。表は全Voice・左右で共有する。
#[path = "table.rs"]
mod table;
use table::TABLE;
const H: f64 = 1.0 / 64.0;
#[inline]
fn coefficients(i: usize) -> [f64; 5] {
    let [g, f, v] = TABLE[i];
    let [_, f1, v1] = TABLE[i + 1];
    [
        g,
        H * f,
        H * H * v / 2.,
        H * (3. * (f1 - f) - H * (2. * v + v1)) / 3.,
        H * (2. * (f - f1) + H * (v + v1)) / 4.,
    ]
}
#[inline]
pub(super) fn eval(x: f64) -> [f64; 4] {
    let a = x.abs();
    let s = x.signum();
    if a >= 16. {
        let d = a - 16.;
        return [
            s * (TABLE[1024][0] + d * (TABLE[1024][1] + d * 0.5)),
            TABLE[1024][1] + d,
            s,
            0.,
        ];
    }
    let u = a / H;
    let i = u as usize;
    let t = u - i as f64;
    let [g, b, c, d, e] = coefficients(i);
    [
        s * (g + t * (b + t * (c + t * (d + t * e)))),
        (b + t * (2. * c + t * (3. * d + t * 4. * e))) / H,
        s * (2. * c + t * (6. * d + t * 12. * e)) / (H * H),
        (6. * d + 24. * e * t) / (H * H * H),
    ]
}
#[inline]
fn local_second(a: f64, b: f64, c: f64) -> Option<f64> {
    if a < 0. || c >= 16. {
        return None;
    }
    let i = (a / H) as usize;
    if i != (c / H) as usize {
        return None;
    }
    let x = a / H - i as f64;
    let y = b / H - i as f64;
    let z = c / H - i as f64;
    let [_, _, p2, p3, p4] = coefficients(i);
    Some(
        2. / (H * H)
            * (p2 + p3 * (x + y + z) + p4 * (x * x + y * y + z * z + x * y + x * z + y * z)),
    )
}
#[inline]
fn secant(a: f64, b: f64, ga: f64, gb: f64) -> f64 {
    let d = b - a;
    if a >= 16. {
        return (a + b) * 0.5 - 16. + TABLE[1024][1];
    }
    if b <= -16. {
        return -(a + b) * 0.5 - 16. + TABLE[1024][1];
    }
    if d < 0.001 {
        let v = eval((a + b) * 0.5);
        return v[1] + v[3] * d * d / 24.;
    }
    (gb - ga) / d
}
#[inline]
pub(super) fn second(
    mut a: f64,
    mut b: f64,
    mut c: f64,
    mut ga: f64,
    mut gb: f64,
    mut gc: f64,
) -> f64 {
    let mut sign = 1.;
    if c <= 0. {
        (a, b, c) = (-c, -b, -a);
        (ga, gb, gc) = (-gc, -gb, -ga);
        sign = -1.;
    }
    if a >= 16. {
        return sign;
    }
    if let Some(y) = local_second(a, b, c) {
        return sign * y;
    }
    let span = c - a;
    if span < 0.001 {
        // 区間境界をまたぐ近接3点は5次精度の三角形求積。重みの合計は1。
        let mut y = 0.225 * eval((a + b + c) / 3.)[2];
        for (t, w) in [
            (0.059715871789770, 0.132394152788506),
            (0.797426985353087, 0.125939180544827),
        ] {
            let s = (1. - t) * 0.5;
            y += w
                * (eval(t * a + s * (b + c))[2]
                    + eval(t * b + s * (a + c))[2]
                    + eval(t * c + s * (a + b))[2]);
        }
        return sign * y;
    }
    sign * 2. * (secant(b, c, gb, gc) - secant(a, b, ga, gb)) / span
}
