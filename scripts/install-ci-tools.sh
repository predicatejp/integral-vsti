#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$project_dir/scripts/ci-versions.sh"

case "$(uname -s)" in
    Linux) native_target=x86_64-unknown-linux-gnu ;;
    MINGW*|MSYS*) native_target=x86_64-pc-windows-msvc ;;
    *) echo '対応対象はLinux/Windows x86_64です。' >&2; exit 2 ;;
esac

install_core() {
    rustup toolchain install stable --profile minimal --no-self-update \
        --component rustfmt,clippy --target "$native_target"
}
install_msrv() {
    rustup toolchain install "$CI_MSRV" --profile minimal --no-self-update
}
install_miri() {
    rustup toolchain install "$CI_NIGHTLY" --profile minimal --no-self-update \
        --component miri,rust-src
    cargo +"$CI_NIGHTLY" miri setup
}
install_coverage() {
    rustup component add --toolchain stable llvm-tools-preview
    cargo +stable install --locked cargo-llvm-cov --version "$CI_CARGO_LLVM_COV"
}
install_audit() {
    cargo +stable install --locked cargo-deny --version "$CI_CARGO_DENY"
}

case "${1:-all}" in
    core) install_core ;;
    msrv) install_msrv ;;
    miri) install_miri ;;
    coverage) install_coverage ;;
    audit) install_audit ;;
    all) install_core; install_msrv; install_miri; install_coverage; install_audit ;;
    *) echo 'Usage: sh scripts/install-ci-tools.sh [all|core|msrv|miri|coverage|audit]' >&2; exit 2 ;;
esac
