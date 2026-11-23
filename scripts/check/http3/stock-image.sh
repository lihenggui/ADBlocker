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

# Print the path to revision 9 of the SDK image, downloading it if needed.
set -euo pipefail
dest=${1:?private directory for the pinned SDK image}
image="${ANDROID_HOME:?}/system-images/android-35/google_apis/x86_64"
if ! grep -Fqx 'Pkg.Revision=9' "$image/source.properties" ||
    ! echo "987818a35792fb62f6ea6aa604d5fa476e1e85c84fc1cbac3979e1ae26965f46  $image/kernel-ranchu" | sha256sum -c --status; then
    mkdir -p "$dest"
    curl -fsSL --retry 3 https://dl.google.com/android/repository/sys-img/google_apis/x86_64-35_r09.zip -o "$dest/image.zip"
    echo "0103e6dab21290c4b9d16550a3ce99476f884eef  $dest/image.zip" | sha1sum -c - >&2
    unzip -q "$dest/image.zip" -d "$dest"
    image="$dest/x86_64"
fi
test -s "$image/system.img"
printf '%s\n' "$image"
