#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
: "${VST3_SDK_DIR:?公式VST3 SDKの展開先をVST3_SDK_DIRで指定してください}"
sh "$project_dir/scripts/build-vst3.sh"
"${CXX:-c++}" -std=c++17 -Wall -Wextra -Werror -I "$VST3_SDK_DIR" \
    "$project_dir/tests/abi-smoke.cpp" -ldl -o "$project_dir/target/abi-smoke"
"$project_dir/target/abi-smoke" \
    "$project_dir/target/vst3/SampleVsti.vst3/Contents/x86_64-linux/SampleVsti.so"
