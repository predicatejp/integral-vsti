// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! DSPを生成する前に、CPUとOSがAVX2/FMAの実行を許可することを確認する。

/// 全体のコンパイル設定に関係なく、検出前は整数命令だけを実行する。
#[cfg(not(miri))]
#[unsafe(naked)]
pub(crate) extern "system" fn supported() -> bool {
    // SAFETY: CPUIDはx86_64の必須命令。XGETBVはXSAVE/OSXSAVE確認後だけ実行する。
    // CPUIDが変更するcallee-savedのRBXは保存し、途中のunwindにも復元位置を示す。
    // 他の変更レジスターは両ABIでcaller-saved。失敗時はfalseの0を返す。
    core::arch::naked_asm!(
        #[cfg(windows)] ".seh_proc {this}",
        #[cfg(not(windows))] ".cfi_startproc\n# {this}",
        "push rbx",
        #[cfg(windows)] ".seh_pushreg rbx\n.seh_endprologue",
        #[cfg(not(windows))] ".cfi_adjust_cfa_offset 8\n.cfi_offset rbx, -16",
        "xor eax, eax",
        "cpuid",
        "cmp eax, 7",
        "jb 2f",
        "mov eax, 1",
        "cpuid",
        // AVX/FMAとOSの対応、およびAVX2のコンパイル時依存SSE3〜SSE4.2を確認する。
        "and ecx, 0x1c181201",
        "cmp ecx, 0x1c181201",
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
        "mov eax, 1",
        "jmp 3f",
        "2:",
        "xor eax, eax",
        "3:",
        "pop rbx",
        #[cfg(not(windows))] ".cfi_adjust_cfa_offset -8\n.cfi_restore rbx",
        "ret",
        #[cfg(windows)] ".seh_endproc",
        #[cfg(not(windows))] ".cfi_endproc",
        this = sym supported,
    );
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
            std::is_x86_feature_detected!("avx2")
                && std::is_x86_feature_detected!("fma")
                && std::is_x86_feature_detected!("sse3")
                && std::is_x86_feature_detected!("ssse3")
                && std::is_x86_feature_detected!("sse4.1")
                && std::is_x86_feature_detected!("sse4.2")
        );
    }
}
