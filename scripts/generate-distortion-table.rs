// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! rustcでビルドし、標準出力をsrc/distortion/table.rsへ保存する。
//! SoftのF1のHermite補間を積分し、整合したF2を生成する。
fn main() {
    println!("// Copyright (C) 2026 Yusuke Oda\n// SPDX-License-Identifier: AGPL-3.0-only");
    println!("// scripts/generate-distortion-table.rsによる生成値。手動編集しない。\n");
    println!("#[rustfmt::skip]\npub(super) static TABLE: [[f64; 3]; 1025] = [");
    let h = 1.0 / 64.0;
    let primitive = |x: f64| x + (-2.0 * x).exp().ln_1p() - std::f64::consts::LN_2;
    let mut integral = 0.0;
    let mut correction = 0.0;
    for i in 0..=1024 {
        let x = f64::from(i) * h;
        let f = primitive(x);
        let v = x.tanh();
        println!("    [{integral:?}, {f:?}, {v:?}],");
        let f1 = primitive(x + h);
        let v1 = (x + h).tanh();
        let b = h * v;
        let c = 3.0 * (f1 - f) - h * (2.0 * v + v1);
        let d = 2.0 * (f - f1) + h * (v + v1);
        // 累積時の丸め誤差を抑えるKahan加算。
        let increment = h * (f + b / 2.0 + c / 3.0 + d / 4.0) - correction;
        let next = integral + increment;
        correction = (next - integral) - increment;
        integral = next;
    }
    println!("];");
}
