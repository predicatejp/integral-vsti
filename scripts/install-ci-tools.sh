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
install_linux_binary() (
    # URL・SHA-256・アーカイブ内の実行ファイルを指定する。
    tool_temp_dir=$(mktemp -d)
    trap 'rm -rf "$tool_temp_dir"' EXIT
    curl --fail --location --silent --show-error --connect-timeout 10 --max-time 60 \
        "$1" -o "$tool_temp_dir/archive.tar.gz"
    printf '%s  %s\n' "$2" "$tool_temp_dir/archive.tar.gz" | sha256sum --check
    tar -xOf "$tool_temp_dir/archive.tar.gz" "$3" > "$tool_temp_dir/binary"
    install -D -m 755 "$tool_temp_dir/binary" "${CARGO_HOME:-$HOME/.cargo}/bin/${3##*/}"
)
install_coverage() {
    rustup component add --toolchain stable llvm-tools-preview
    if [ "$native_target" = x86_64-unknown-linux-gnu ]; then
        install_linux_binary \
            "https://github.com/taiki-e/cargo-llvm-cov/releases/download/v$CI_CARGO_LLVM_COV/cargo-llvm-cov-x86_64-unknown-linux-musl.tar.gz" \
            "$CI_CARGO_LLVM_COV_LINUX_SHA256" cargo-llvm-cov
    else
        cargo +stable install --locked cargo-llvm-cov --version "$CI_CARGO_LLVM_COV"
    fi
}
install_audit() {
    if [ "$native_target" = x86_64-unknown-linux-gnu ]; then
        tool_archive="cargo-deny-$CI_CARGO_DENY-x86_64-unknown-linux-musl"
        install_linux_binary \
            "https://github.com/EmbarkStudios/cargo-deny/releases/download/$CI_CARGO_DENY/$tool_archive.tar.gz" \
            "$CI_CARGO_DENY_LINUX_SHA256" "$tool_archive/cargo-deny"
    else
        cargo +stable install --locked cargo-deny --version "$CI_CARGO_DENY"
    fi
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
