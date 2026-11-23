#!/usr/bin/env bash
#
# Copyright (C) 2026 Mercury Li
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License version 3 as
# published by the Free Software Foundation.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program. If not, see <https://www.gnu.org/licenses/>.

# Build the HTTP/3 test executable for the device CI emulator.
set -euo pipefail
cd "$(dirname "$0")/.."
sdk=${ANDROID_HOME:-$HOME/Android/Sdk}
ndk=${ANDROID_NDK_HOME:-$(find "$sdk/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)}
bin="$ndk/toolchains/llvm/prebuilt/$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64/bin"
abi=${1:-x86_64}
case "$abi" in
    x86_64) target=x86_64-linux-android ;;
    arm64-v8a) target=aarch64-linux-android ;;
    *) echo "Unsupported fixture ABI: $abi" >&2; exit 2 ;;
esac
cc="$bin/${target}31-clang"
key=${target//-/_}
export "CC_$key=$cc" "AR_$key=$bin/llvm-ar"
export "CARGO_TARGET_$(echo "$key" | tr '[:lower:]' '[:upper:]')_LINKER=$cc"
cargo build --release --locked -p abd-vpn --example check_http3 --target "$target"
mkdir -p out
install -m 755 "${CARGO_TARGET_DIR:-target}/$target/release/examples/check_http3" "out/check-http3-$abi"
