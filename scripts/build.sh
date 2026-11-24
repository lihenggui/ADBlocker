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

# Cross-compiles abd for every architecture the module supports, then builds
# the APK, which is both the app and the flashable root module.
#
# Requires: rustup targets aarch64-linux-android and x86_64-linux-android,
# an Android NDK, a JDK and the Android SDK.
#
# With ADBLOCKER_NATIVE_ONLY=1 it stops once the native components are
# staged in out/, for a build system that runs Gradle itself (F-Droid).
set -euo pipefail

cd "$(dirname "$0")/.."

# The architectures the module supports, and the one place they are listed:
# the installer accepts a device whose $ARCH has a daemon in the archive.
# arm64 is what devices run; x86_64 is what the Android emulator runs.
TARGETS=(aarch64-linux-android x86_64-linux-android)

# Rust target -> the architecture name module installers set in $ARCH.
arch_of() {
    case "$1" in
    aarch64-*) echo arm64 ;;
    x86_64-*) echo x64 ;;
    *) die "no module architecture name for $1" ;;
    esac
}
API=31 # Android 12, the oldest release the module is meant for.

die() {
    echo "error: $*" >&2
    exit 1
}

export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
# The NDK of scripts/ndk-version, or $ANDROID_NDK_HOME. Native code built
# with another NDK differs from a release's, which others build again and
# compare (F-Droid): a release takes that one only.
ndk_version="$(<scripts/ndk-version)"
ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/$ndk_version}"
[[ -d "$ANDROID_NDK_HOME" ]] ||
    die "Android NDK not found at $ANDROID_NDK_HOME; install ndk;$ndk_version or set ANDROID_NDK_HOME"
[[ -f "$ANDROID_NDK_HOME/source.properties" ]] ||
    die "$ANDROID_NDK_HOME is not an NDK: it has no source.properties"
ndk_revision="$(sed -n 's/^Pkg.Revision *= *//p' "$ANDROID_NDK_HOME/source.properties")"
if [[ "$ndk_revision" != "$ndk_version" ]]; then
    [[ "${ADBLOCKER_REQUIRE_RELEASE_SIGNING:-}" != 1 ]] ||
        die "a release is built with NDK $ndk_version, not $ndk_revision"
    echo "warning: NDK $ndk_revision, not $ndk_version: the native code differs from a release's" >&2
fi

host_tag="$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64"
bin="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin"

# A panic's location names the source file, of a crate in Cargo's home
# or of this checkout: paths of the machine that built it, which would
# make one build of a release differ from another's (F-Droid's), and
# carry its user's name. Named from fixed roots instead.
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$PWD=/adblocker"

target_args=()
for target in "${TARGETS[@]}"; do
    clang="$bin/${target}${API}-clang"
    [[ -x "$clang" ]] || die "no NDK toolchain for $target at $bin"
    # cargo and the cc crate take the linker and compilers from variables
    # named after the target. (tr rather than ${var^^}: macOS ships bash 3.)
    target_env="${target//-/_}"
    export "CARGO_TARGET_$(echo "$target_env" | tr '[:lower:]' '[:upper:]')_LINKER=$clang"
    export "CC_${target_env}=$clang"
    export "AR_${target_env}=$bin/llvm-ar"
    target_args+=(--target "$target")
done

# One cargo run builds every target, sharing the host-side work.
cargo build --release --package abd "${target_args[@]}" "$@"

# The engine as a library, for filtering without root, which the app does
# from Android 10: linked for that API rather than the daemon's.
VPN_API=29
for target in "${TARGETS[@]}"; do
    clang="$bin/${target}${VPN_API}-clang"
    [[ -x "$clang" ]] || die "no NDK toolchain for $target at $bin"
    target_env="${target//-/_}"
    export "CARGO_TARGET_$(echo "$target_env" | tr '[:lower:]' '[:upper:]')_LINKER=$clang"
    export "CC_${target_env}=$clang"
done
cargo build --release --package abd-vpn "${target_args[@]}" "$@"

version="$(sed -n 's/^version=//p' module/module.prop)"

# The daemons, one per architecture, where Gradle packages them (out/abd-<arch>,
# android/build-logic FlashableModuleConventionPlugin) as bin/<arch>/abd.
mkdir -p out
rm -f out/abd-*
# The engine library, where the app packages it as lib/<abi>/libabd_vpn.so.
rm -rf out/jni
for target in "${TARGETS[@]}"; do
    case "$target" in
    aarch64-*) abi=arm64-v8a ;;
    x86_64-*) abi=x86_64 ;;
    esac
    mkdir -p "out/jni/$abi"
    cp "target/$target/release/libabd_vpn.so" "out/jni/$abi/libabd_vpn.so"
    echo "built: out/jni/$abi/libabd_vpn.so ($(du -h "out/jni/$abi/libabd_vpn.so" | cut -f1))"
done
for target in "${TARGETS[@]}"; do
    daemon="out/abd-$(arch_of "$target")"
    cp "target/$target/release/abd" "$daemon"
    echo "built: $daemon ($(du -h "$daemon" | cut -f1))"
done
if [[ "${ADBLOCKER_NATIVE_ONLY:-}" == 1 ]]; then
    exit 0
fi

# The APK is the app and the flashable module at once: the module's files
# are at the root of the archive, where the root managers look for them.
gradle_args=(--quiet :app:assembleRelease)
if [[ "${ADBLOCKER_REQUIRE_RELEASE_SIGNING:-}" == 1 ]]; then
    gradle_args+=(--no-configuration-cache :app:dependencyGuard :app:licenseeAndroidRelease
        :app:checkReleaseBadging :app:lintRelease)
fi
(cd android && ./gradlew "${gradle_args[@]}")

# Named .apk.zip: root managers take a .zip, and removing the .zip leaves
# an APK to install as an app.
zip="out/adblocker-$version.apk.zip"
rm -f "$zip"
cp android/app/build/outputs/apk/release/app-release.apk "$zip"
echo "built: $zip ($(du -h "$zip" | cut -f1))"

mapping="out/adblocker-$version-mapping.txt"
cp android/app/build/outputs/mapping/release/mapping.txt "$mapping"
echo "built: $mapping"
