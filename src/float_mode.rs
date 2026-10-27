// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 音声処理スレッドの非正規化数をゼロとして扱い、終了時にMXCSR全体を戻す。
//! MiriではCPUの制御レジスタを操作せず、DSPのメモリ安全性だけを検証する。

/// 同じスレッド内のスコープでだけ使用する浮動小数点環境の保存値。
pub struct FloatMode {
    /// 呼び出し元のMXCSR。丸め・例外マスク・ステータスも含む。
    previous: u32,
    /// 別スレッドでの復元を型レベルで禁止する。
    _thread: std::marker::PhantomData<*mut ()>,
}

impl FloatMode {
    pub fn flush_subnormals() -> Self {
        let previous = read();
        write(previous | (1 << 15) | (1 << 6));
        Self {
            previous,
            _thread: std::marker::PhantomData,
        }
    }
}

impl Drop for FloatMode {
    fn drop(&mut self) {
        write(self.previous);
    }
}

pub(crate) fn read() -> u32 {
    #[cfg(not(miri))]
    {
        let mut value = 0;
        // SAFETY: 対応対象はSSE2を備えるx86_64。有効な4バイト領域へ読み出す。
        unsafe {
            std::arch::asm!("stmxcsr [{p}]", p = in(reg) &mut value, options(nostack, preserves_flags));
        }
        value
    }
    #[cfg(miri)]
    {
        0
    }
}

fn write(value: u32) {
    #[cfg(not(miri))]
    // SAFETY: 同じスレッドから読み出したMXCSRのFTZ/DAZのみ変更し、予約ビットは保持する。
    // メモリ副作用を指定して、DSP状態の読み書きがスコープを越えないようにする。
    unsafe {
        std::arch::asm!("ldmxcsr [{p}]", p = in(reg) &value, options(nostack, preserves_flags));
    }
    #[cfg(miri)]
    let _ = value;
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use std::hint::black_box;

    #[test]
    fn restores_all_modes_and_flushes_arithmetic() {
        let original = read();
        let _restore = FloatMode {
            previous: original,
            _thread: std::marker::PhantomData,
        };
        for flags in [0, 1 << 6, 1 << 15, (1 << 6) | (1 << 15)] {
            // マスクを維持し、丸めモードとステータスも復元対象に含める。
            let host = (original & !0xe040) | flags | (1 << 13) | 0x20;
            write(host);
            {
                let _mode = FloatMode::flush_subnormals();
                assert_eq!(read(), host | 0x8040);
                let subnormal = black_box(f64::MIN_POSITIVE / 2.0);
                assert_eq!(black_box(subnormal) * black_box(2.0), 0.0);
                assert_eq!(black_box(f64::MIN_POSITIVE) * black_box(0.1), 0.0);
                let outer = read();
                {
                    let _nested = FloatMode::flush_subnormals();
                }
                assert_eq!(read(), outer);
            }
            assert_eq!(read(), host);
        }
    }
}
