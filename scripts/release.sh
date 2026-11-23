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

# Build and bundle the corresponding source of one tagged release.
set -euo pipefail

cd "$(dirname "$0")/.."

die() {
    echo "error: $*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die 'usage: scripts/release.sh <tag>'
tag="$1"
version="$(sed -n 's/^version=//p' module/module.prop)"
[[ "$tag" == v* && "$tag" != */* && "$tag" == "$version" ]] ||
    die "tag must match module/module.prop version $version"
commit="$(git rev-parse --verify "refs/tags/$tag^{commit}")" || die "missing tag $tag"
[[ "$commit" == "$(git rev-parse HEAD)" ]] || die "check out tag $tag"
[[ -z "$(git status --porcelain)" ]] || die 'build the release from a clean checkout of the tag'
[[ -n "${ADBLOCKER_SIGNING_STORE_FILE:-}" || -f android/keystore.properties ]] ||
    die 'configure the release keystore before building'

export ADBLOCKER_REQUIRE_RELEASE_SIGNING=1
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
    ANDROID_NDK_HOME="$(find "$ANDROID_HOME/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)"
    export ANDROID_NDK_HOME
fi
tools="$(find "$ANDROID_HOME/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)"
[[ -x "$tools/apksigner" ]] || die 'apksigner is missing from the Android SDK'

./scripts/build.sh --locked
artifact="adblocker-$tag"
signers="$("$tools/apksigner" verify --print-certs "out/$artifact.apk.zip")"
if grep -Eq '^Signer #[0-9]+ certificate DN:.*CN=Android Debug([, ]|$)' <<<"$signers"; then
    die 'the APK was signed with an Android Debug certificate'
fi
build_info="$(mktemp -d)"
trap 'rm -rf -- "$build_info"' EXIT
source_name="$artifact-source"
git archive --format=tar --prefix="$source_name/" "$commit" | tar -xf - -C "$build_info"
source_dir="$build_info/$source_name"
mkdir -p "$source_dir/.cargo" "$source_dir/release-info"
(
    cd "$source_dir"
    # Cargo verifies these registry packages against the tagged Cargo.lock.
    cargo vendor --locked --versioned-dirs vendor > .cargo/config.toml
)
cp android/app/build/reports/licensee/androidRelease/artifacts.json "$source_dir/release-info/android-licenses.json"

# Maven sources JARs do not include the AndroidX native build inputs.
androidx=https://android.googlesource.com/platform/frameworks/support
native_dir="$source_dir/android-vendor"
mkdir "$native_dir"
bundle_androidx_source() {
    local coordinate="$1" revision="$2" module="$3" native_file="$4"
    local artifact_id version archive
    IFS=: read -r _ artifact_id version <<<"$coordinate"
    grep -Fxq "$coordinate" android/app/dependencies/releaseRuntimeClasspath.txt ||
        die "update the bundled source revision for $artifact_id"
    archive="$native_dir/$artifact_id-$version.tar.gz"
    curl --fail --silent --show-error --location --retry 3 --max-time 120 \
        "$androidx/+archive/$revision/$module.tar.gz" --output "$archive"
    tar -tzf "$archive" "$native_file" >/dev/null
}

native_libraries="$(unzip -Z1 "out/$artifact.apk.zip" | awk -F/ '$1 == "lib" && NF == 3 && $3 ~ /[.]so$/ { print $3 }' | sort -u)"
[[ "$native_libraries" == $'libabd_vpn.so\nlibandroidx.graphics.path.so\nlibdatastore_shared_counter.so' ]] ||
    die 'review the sources for the native libraries packaged in the APK'
bundle_androidx_source androidx.graphics:graphics-path:1.1.0 \
    1c44f0be720ff16cd30357ad2375239bf97a1da8 graphics/graphics-path src/main/cpp/CMakeLists.txt
bundle_androidx_source androidx.datastore:datastore-core-android:1.3.0 \
    de4cda56f812357e582b3e1aa32bc737636614a2 datastore/datastore-core src/androidMain/cpp/shared_counter.h
cp licenses/Apache-2.0.txt "$native_dir/LICENSE.txt"

{
    printf 'Tag: %s\nCommit: %s\n' "$tag" "$commit"
    rustc --version --verbose
    java -version 2>&1
    sed -n '/^Pkg.Revision/p' "$ANDROID_NDK_HOME/source.properties"
    sed -n '/^distributionUrl/p; /^distributionSha256Sum/p' android/gradle/wrapper/gradle-wrapper.properties
    printf '%s\n' "$signers"
} > "$source_dir/release-info/build.txt"
tar -czf "$build_info/$source_name.tar.gz" -C "$build_info" "$source_name"
mv "$build_info/$source_name.tar.gz" out/
(cd out && sha256sum "$artifact.apk.zip" "$artifact-mapping.txt" "$source_name.tar.gz" > "$artifact-SHA256SUMS")
