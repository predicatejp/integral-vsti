// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! rustcでビルドし、標準出力をsrc/oscillator_tables.rsへ保存する。
//! 通常のビルド・音声処理では実行しない。

fn main() {
    println!("// Copyright (C) 2026 Yusuke Oda\n// SPDX-License-Identifier: AGPL-3.0-only");
    println!("// scripts/generate-oscillator-tables.rsによる生成値。手動編集しない。\n");
    for (name, values) in [
        (
            "DETUNE",
            (0..=200)
                .map(|i| 2.0_f64.powf((f64::from(i) - 100.0) / 1200.0))
                .collect::<Vec<_>>(),
        ),
        (
            "SINE",
            (0..=1024)
                .map(|i| (std::f64::consts::FRAC_PI_2 * f64::from(i) / 1024.0).sin())
                .collect(),
        ),
    ] {
        println!(
            "#[rustfmt::skip]\npub(super) static {name}: [f64; {}] = [",
            values.len()
        );
        for row in values.chunks(4) {
            println!(
                "    {},",
                row.iter()
                    .map(|v| format!("{v:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        println!("];");
        if name == "DETUNE" {
            println!();
        }
    }
}
