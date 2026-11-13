#!/usr/bin/env bash
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/ci-versions.sh
export MIRIFLAGS=-Zmiri-no-extra-rounding-error

# ABIの借用・所有権、IBStream、編集キュー、unsafeな数値変換とSIMDを検査する。
# 長時間の音声走査は通常のdebug/releaseテストで実行する。
listing=$(cargo +"$CI_NIGHTLY" miri test --locked --lib -- --list)
mapfile -t cases < <(printf '%s\n' "$listing" | sed -n -E \
  '/^(tests::|parameters::tests::|editor::edits::tests::)/s/: test$//p')
(( ${#cases[@]} > 0 ))
cargo +"$CI_NIGHTLY" miri test --locked --lib -- --exact "${cases[@]}" \
  oscillator::wavetable::tests::generated_table_positions_fit_fast_integer_conversion \
  oscillator::adaa::tests::folded_lookup_covers_valid_phase_endpoints \
  oscillator::tests::phase_modulation_wraps_without_changing_stored_phase \
  oscillator::tests::invalid_inputs_preserve_state_and_never_emit_nonfinite_samples \
  filter::tests::stereo_channels_share_coefficients_but_not_history
