#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$(uname -s)" != Linux ] || [ "$(uname -m)" != x86_64 ]; then
    echo '現在のビルドスクリプトはLinux x86_64向けです。' >&2
    exit 1
fi

cargo build --release --locked --manifest-path "$project_dir/Cargo.toml" \
    --target x86_64-unknown-linux-gnu --target-dir "$project_dir/target"

bundle_dir="$project_dir/target/vst3/SampleVsti.vst3"
mkdir -p "$bundle_dir/Contents/x86_64-linux"
cp "$project_dir/target/x86_64-unknown-linux-gnu/release/libsample_vsti.so" \
    "$bundle_dir/Contents/x86_64-linux/SampleVsti.so"
license_dir="$bundle_dir/Contents/Resources/Licenses"
mkdir -p "$license_dir"
cp "$project_dir/LICENSE" "$project_dir/COPYRIGHT" \
    "$project_dir/THIRD_PARTY_NOTICES.txt" "$license_dir/"
echo "$bundle_dir"
