// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! DSPを生成する前に、CPUとOSがAVX2/FMAの実行を許可することを確認する。

#[cfg(not(miri))]
macro_rules! gate {
    ($success:literal $(, $operand:ident = sym $symbol:path)*) => {
    // SAFETY: CPUIDはx86_64の必須命令。XGETBVはXSAVE/OSXSAVE確認後だけ実行する。
    // RBXはR11へ保存して復元し、変更する他のレジスターは両ABIでcaller-saved。
    // スタックに触れず、失敗時はnull/falseの表現である0を返す。
    core::arch::naked_asm!(
        "mov r11, rbx",
        "xor eax, eax",
        "cpuid",
        "cmp eax, 7",
        "jb 2f",
        "mov eax, 1",
        "cpuid",
        // FMA、XSAVE、OSXSAVE、AVXの4ビットが必要。
        "and ecx, 0x1c001000",
        "cmp ecx, 0x1c001000",
        "jne 2f",
        "xor ecx, ecx",
        "xgetbv",
        // OSがXMM/YMMの両状態を保存・復元することを確認する。
        "and eax, 6",
        "cmp eax, 6",
        "jne 2f",
        "mov eax, 7",
        "xor ecx, ecx",
        "cpuid",
        "test ebx, 32",
        "jz 2f",
        "mov rbx, r11",
        $success,
        "2:",
        "xor eax, eax",
        "mov rbx, r11",
        "ret",
        $($operand = sym $symbol,)*
    );
    };
}

#[cfg(not(miri))]
pub(crate) use gate;

/// 全体のコンパイル設定に関係なく、検出前は整数命令だけを実行する。
#[cfg(not(miri))]
#[unsafe(naked)]
pub(crate) extern "system" fn supported() -> bool {
    gate!("mov eax, 1; ret");
}

// Miriは実CPUを起動しない。命令セットの拒否はロード試験で検証する。
#[cfg(miri)]
pub(crate) extern "system" fn supported() -> bool {
    true
}

#[cfg(all(test, not(miri)))]
mod tests {
    #[test]
    fn detects_the_same_requirements_as_the_standard_library() {
        assert_eq!(
            super::supported(),
            std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
        );
    }
}
