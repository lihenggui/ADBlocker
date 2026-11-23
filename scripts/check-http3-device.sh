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

# Run on a disposable root-capable emulator. The HTTP/3 and TCP origins
# run across a veth pair in a separate network namespace.
set -euo pipefail
umask 077
mode=${1:-}
if [[ $# != 1 || ( $mode != root && $mode != vpn ) ]]; then
    echo "usage: $0 root | vpn" >&2
    exit 2
fi
fail() { echo "FAIL  $*" >&2; exit 1; }
cd "$(dirname "$0")/.."
: "${ANDROID_SERIAL:?choose a disposable emulator}"
adb root >/dev/null
adb wait-for-device
[[ $(adb shell getprop ro.kernel.qemu | tr -d '\r') == 1 && $(adb shell id -u | tr -d '\r') == 0 ]]
package=li.mercury.adblocker.check.http3
control=$package.control
# The module in a root manager once it has its daemon, or else the one
# installed without a manager (module/direct.sh).
abd=$(adb shell 'm=/data/adb/modules/adblocker; [ -e $m/bin/abd ] || m=/data/adb/adblocker_direct; echo $m/bin/abd' | tr -d '\r')
[[ $abd == /data/adb/*/bin/abd ]] || fail "could not ask the device where the module is"
for id in "$package" "$control"; do
    existing=$(adb shell pm list packages --user all -u "$id" | tr -d '\r') || fail "could not check existing fixture packages"
    if grep -Fqx "package:$id" <<<"$existing"; then fail "fixture package $id already exists"; fi
done
sdk=${ANDROID_HOME:-$HOME/Android/Sdk}
tools=$(find "$sdk/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
platform=$(find "$sdk/platforms" -name android.jar | sort -V | tail -1)
abi=$(adb shell getprop ro.product.cpu.abi | tr -d '\r')
bin="out/check-http3-$abi"
[[ -x $bin ]] || { echo "Run scripts/build-http3-check.sh $abi first" >&2; exit 1; }
src=scripts/check/http3
work=$(mktemp -d)
stage=$(adb shell 'mktemp -d /data/local/tmp/abd-http3.XXXXXX' | tr -d '\r')
[[ $stage =~ ^/data/local/tmp/abd-http3\.[[:alnum:]]+$ ]]
list="/data/adb/adblocker/rules/${stage##*/}.txt"
installed=()
cleanup() {
    local status=$?
    if ((status != 0)); then
        for log in "$work/origin" "$work/tcp-origin"; do
            if [[ -s $log ]]; then cat "$log" >&2; fi
        done
    fi
    if [[ -n ${origin:-} ]]; then kill "$origin" 2>/dev/null || true; wait "$origin" 2>/dev/null || true; fi
    if [[ -n ${tcp_origin:-} ]]; then kill "$tcp_origin" 2>/dev/null || true; wait "$tcp_origin" 2>/dev/null || true; fi
    if [[ -n ${previous_urls:-} ]]; then adb shell "$abd url-filtering $previous_urls" >/dev/null || true; fi
    if [[ $mode == root ]]; then adb shell "$abd app remove $package; rm -f $list; $abd reload" >/dev/null || true; fi
    adb shell "if test -f $stage/net; then sh $stage/net stop $stage; fi; rm -rf $stage" >/dev/null || true
    for id in "${installed[@]}"; do adb shell pm uninstall --user 0 "$id" >/dev/null || true; done
    rm -rf "$work"
}
trap cleanup EXIT
mkdir -p "$work/assets" "$work/classes" "$work/lib/$abi"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 -subj /CN=fixture.example \
    -addext subjectAltName=DNS:fixture.example -addext basicConstraints=critical,CA:FALSE \
    -keyout "$work/leaf.pem" -out "$work/cert.pem" 2>/dev/null
certificate_ready=$(date +%s)
openssl x509 -in "$work/cert.pem" -outform DER -out "$work/assets/cert.der"
openssl pkcs8 -topk8 -nocrypt -in "$work/leaf.pem" -outform DER -out "$work/key.der"
adb push "$work/assets/cert.der" "$stage/cert.der" >/dev/null
javac --release 11 -Xlint:-options -cp "$platform" -d "$work/classes" "$src/Probe.java"
find "$work/classes" -name '*.class' -exec "$tools/d8" --min-api 31 --lib "$platform" --output "$work" {} +
cp "$bin" "$work/lib/$abi/libh3check.so"
keytool -genkeypair -keystore "$work/test.jks" -storepass android -keypass android -alias check \
    -keyalg RSA -keysize 2048 -validity 1 -dname CN=Fixture >/dev/null 2>&1
for id in "$package" "$control"; do
    "$tools/aapt2" link -o "$work/app.apk" -I "$platform" --manifest "$src/AndroidManifest.xml" -A "$work/assets" --rename-manifest-package "$id"
    (cd "$work" && zip -q app.apk classes.dex "lib/$abi/libh3check.so")
    "$tools/zipalign" -f 4 "$work/app.apk" "$work/aligned.apk"
    "$tools/apksigner" sign --ks "$work/test.jks" --ks-pass pass:android --out "$work/check.apk" "$work/aligned.apk"
    adb install -t --user 0 "$work/check.apk" >/dev/null
    installed+=("$id")
done
adb push "$bin" "$stage/check" >/dev/null
adb push "$work/key.der" "$stage/key.der" >/dev/null
adb push "$src/network.sh" "$stage/net" >/dev/null
# An emulator can lag behind the host clock after resuming.
deadline=$((SECONDS + 45))
until device_time=$(adb shell date +%s) && ((device_time >= certificate_ready)); do
    ((SECONDS < deadline)) || fail "the emulator clock is behind the fixture certificate"
    sleep 1
done
adb shell "chmod 700 $stage/check; chmod 600 $stage/key.der; sh $stage/net start $stage $mode"
if [[ $mode == root ]]; then
    before=$(adb shell "$abd status --json")
    previous_urls=$(jq -r '.url_filtering | if . then "on" else "off" end' <<<"$before")
    previous_count=$(jq '.filtering_apps // 0' <<<"$before")
    adb shell "$abd url-filtering on"
    adb shell "umask 077; mkdir -p /data/adb/adblocker/rules; echo '||fixture.example/ads/' >$list; $abd app add $package"
    for ((i=0; i<50; i++)); do
        if adb shell "$abd status --json" | jq -e --argjson previous "$previous_count" '.filtering_apps > $previous and .ipv6' >/dev/null; then break; fi
        sleep .1
    done
    ((i < 50)) || fail "Root capture did not become ready for both families"
fi
fetch() {
    local ipv6=$1 caller=$2 probe=$3 result i
    adb shell "run-as $caller rm -f files/result && am start -S -W -n $caller/$package.Probe --ez ipv6 $ipv6 --es probe $probe" >/dev/null
    for ((i=0; i<150; i++)); do
        result=$(adb shell "run-as $caller cat files/result 2>/dev/null || true" | tr -d '\r')
        if [[ -n $result ]]; then printf '%s\n' "$result"; return 0; fi
        sleep .1
    done
    echo "The HTTP/3 fixture returned no result" >&2
    return 1
}
start_origin() {
    local kind=$1 pid i
    adb shell "sh $stage/net $kind $stage $family" >"$work/$kind" 2>&1 &
    pid=$!
    if [[ $kind == origin ]]; then origin=$pid; else tcp_origin=$pid; fi
    for ((i=0; i<50; i++)); do
        grep -q '^listening ' "$work/$kind" && return 0
        kill -0 "$pid" || break
        sleep .1
    done
    fail "$kind did not start"
}
companion=li.mercury.adblocker
for family in 4 6; do
    ipv6=false
    if [[ $family == 6 ]]; then ipv6=true; fi
    start_origin tcp-origin
    start_origin origin
    if [[ $mode == root ]]; then
        result=$(fetch "$ipv6" "$package" dual-fetch)
        grep -Eq '^refused in [0-9]{1,3} ms$' <<<"$result" || fail "$result"
        grep -Fxq 'tcp /ads/banner 403' <<<"$result" || fail "$result"
        grep -Fxq 'tcp /articles/today 200' <<<"$result" || fail "$result"
        printf '%s\n' "$result"
        result=$(fetch "$ipv6" "$control" fetch)
        grep -Fxq 'h3 /ads/banner 200' <<<"$result" || fail "$result"
        grep -Fxq 'h3 /articles/today 200' <<<"$result" || fail "$result"
    else
        adb shell am instrument -w -e ipv6 "$ipv6" -e class "$companion.Http3DeviceTest" \
            "$companion.test/androidx.test.runner.AndroidJUnitRunner" | tee "$work/instrument"
        grep -Eq '^OK \(1 test\)' "$work/instrument" || fail "VPN instrumentation failed"
    fi
    wait "$origin"
    origin=
    received=$(sed -n 's/^origin received //p' "$work/origin" | tr -d '\r' | sort)
    [[ $received == $'/ads/banner\n/articles/today' ]] || fail "unexpected HTTP/3 requests: $received"
    if grep -q '^origin received ' "$work/tcp-origin"; then
        wait "$tcp_origin"
        [[ $(sed -n 's/^origin received //p' "$work/tcp-origin" | tr -d '\r') == /articles/today ]]
        echo "ok    $mode IPv$family: fast QUIC refusal, banner 403 and article 200 over TCP, control on HTTP/3"
    elif [[ $mode == vpn && $family == 6 ]] && grep -q 'VPN has no IPv6 route' "$work/instrument"; then
        kill "$tcp_origin" 2>/dev/null || true
        wait "$tcp_origin" 2>/dev/null || true
        echo "skip  VPN IPv6 filtering: underlying network has no IPv6 route (control used HTTP/3)"
    else
        fail "no allowed TCP request at the origin"
    fi
    tcp_origin=
done
