# SPDX-License-Identifier: GPL-3.0-only
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

# Sourced by check-device.sh. Use the existing isolated HTTP/3 fixture
# network and build the same client in its WebView mode.
check_cosmetic() {
    : "${ANDROID_SERIAL:?choose a rooted test emulator}"
    [[ $(adb shell getprop ro.kernel.qemu | tr -d '\r') == 1 && $(adb shell id -u | tr -d '\r') == 0 ]] ||
        die "the cosmetic check needs a rooted emulator"
    local abi status serial_stage
    abi=$(adb shell getprop ro.product.cpu.abi | tr -d '\r')
    [[ -x out/check-http3-$abi ]] || die "run scripts/build-http3-check.sh $abi first"
    status=$(adb shell "$ABD status --json")
    jq -e '.running == true' <<<"$status" >/dev/null &&
        adb exec-out cat "$DATA/state" | jq -e '.exclusions_ready == true' >/dev/null ||
        die "install the module and its HTTPS exclusions first"
    if adb shell pm list packages --user all -u "$PACKAGE" | tr -d '\r' | grep -Fxq "package:$PACKAGE"; then
        die "the check package already exists"
    fi
    serial_stage=$(adb shell 'umask 077; mktemp -d /data/local/tmp/abd-cosmetic.XXXXXX' | tr -d '\r')
    [[ $serial_stage =~ ^/data/local/tmp/abd-cosmetic\.[[:alnum:]]+$ ]] || die "could not create fixture directory"
    cosmetic_stage=$serial_stage
    cosmetic_cleanup() {
        local result=$?
        trap - EXIT
        if [[ ${cosmetic_installed:-} == 1 ]]; then
            adb shell pm uninstall --user 0 "$PACKAGE" >/dev/null || result=1
        fi
        if [[ ${cosmetic_daemon_replaced:-} == 1 ]]; then
            adb shell "ABD_DATA=$cosmetic_stage/data $ABD module-stop" || result=1
        fi
        if [[ ${cosmetic_hosts_mounted:-} == 1 ]]; then
            adb shell umount /system/etc/hosts || result=1
        fi
        if [[ -n ${cosmetic_origin_job:-} ]]; then
            adb shell "if test -f $cosmetic_stage/origin.pid; then kill \$(cat $cosmetic_stage/origin.pid) 2>/dev/null || true; fi"
            kill "$cosmetic_origin_job" 2>/dev/null || true
            wait "$cosmetic_origin_job" 2>/dev/null || true
        fi
        if [[ ${cosmetic_net_created:-} == 1 ]]; then
            adb shell "sh $cosmetic_stage/net stop $cosmetic_stage" || result=1
        fi
        if [[ ${cosmetic_daemon_replaced:-} == 1 ]]; then
            adb shell "setsid sh $MODPATH/service.sh </dev/null >/dev/null 2>&1" || result=1
            for ((i=0; i<50; i++)); do
                if adb shell "$ABD status --json" | jq -e '.running' >/dev/null; then break; fi
                sleep .2
            done
            ((i < 50)) || result=1
        fi
        if ((result != 0)); then
            cat "$work/cosmetic-daemon.log" "$work/cosmetic-origin.log" >&2 2>/dev/null || true
        fi
        adb shell "rm -rf $cosmetic_stage" >/dev/null || result=1
        rm -rf "$work"
        exit "$result"
    }
    trap cosmetic_cleanup EXIT

    build_check_apk scripts/check/app check
    adb install --user 0 "$work/check.apk" >/dev/null
    cosmetic_installed=1

    # Give only the fixture daemon the upstream CA, using a private copy
    # of the system store in its own mount namespace. The regular module
    # and its data are restored after the fixture daemon stops.
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1         -subj '/CN=ADBlocker fixture CA' -keyout "$work/ca.key" -out "$work/ca.pem"         -addext 'basicConstraints=critical,CA:TRUE' 2>/dev/null
    openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes         -subj /CN=fixture.example -keyout "$work/leaf.key" -out "$work/leaf.csr" 2>/dev/null
    printf '%s\n' 'subjectAltName=DNS:fixture.example' 'basicConstraints=critical,CA:FALSE' > "$work/leaf.ext"
    openssl x509 -req -in "$work/leaf.csr" -CA "$work/ca.pem" -CAkey "$work/ca.key"         -CAcreateserial -days 1 -extfile "$work/leaf.ext" -outform DER -out "$work/leaf.der" 2>/dev/null
    openssl pkcs8 -topk8 -nocrypt -in "$work/leaf.key" -outform DER -out "$work/leaf-key.der"
    local ca_name
    ca_name=$(openssl x509 -in "$work/ca.pem" -subject_hash_old -noout).0
    adb push scripts/check/http3/network.sh "$cosmetic_stage/net" >/dev/null
    adb push "out/check-http3-$abi" "$cosmetic_stage/check" >/dev/null
    adb push "$work/leaf.der" "$work/leaf-key.der" "$cosmetic_stage" >/dev/null
    adb shell "mkdir -p $cosmetic_stage/system-roots $cosmetic_stage/data/rules
        cp -a /apex/com.android.conscrypt/cacerts/. $cosmetic_stage/system-roots/
        test ! -e $cosmetic_stage/system-roots/$ca_name"
    adb push "$work/ca.pem" "$cosmetic_stage/system-roots/$ca_name" >/dev/null
    adb shell "chmod 700 $cosmetic_stage/check; chmod 600 $cosmetic_stage/leaf-key.der
        chmod 755 $cosmetic_stage/system-roots
        chcon -R u:object_r:system_security_cacerts_file:s0 $cosmetic_stage/system-roots
        cp -r $DATA/exclusions $cosmetic_stage/data/exclusions
        cp /system/etc/hosts $cosmetic_stage/hosts
        printf '\n198.18.0.2 fixture.example\n' >> $cosmetic_stage/hosts
        chmod 644 $cosmetic_stage/hosts
        chcon u:object_r:system_file:s0 $cosmetic_stage/hosts"
    adb shell "umask 077; cat > $cosmetic_stage/data/rules/cosmetic.txt" <<'RULES'
fixture.example##+js(set-constant, fixtureConstant, 42)
fixture.example###advert:has-text(Sponsored fixture)
RULES
    cosmetic_net_created=1
    adb shell "sh $cosmetic_stage/net start $cosmetic_stage root"
    adb shell "mount -o bind $cosmetic_stage/hosts /system/etc/hosts"
    cosmetic_hosts_mounted=1
    adb shell "echo \$\$ >$cosmetic_stage/origin.pid; exec nsenter --net=/proc/\$(cat $cosmetic_stage/ns.pid)/ns/net -- $cosmetic_stage/check cosmetic-origin \
        198.18.0.2:443 $cosmetic_stage/leaf.der $cosmetic_stage/leaf-key.der" >"$work/cosmetic-origin.log" 2>&1 &
    cosmetic_origin_job=$!
    for ((i=0; i<50; i++)); do
        if grep -q '^listening ' "$work/cosmetic-origin.log"; then break; fi
        sleep .2
    done
    ((i < 50)) || die "the local HTTPS fixture did not start"

    local fixture_abd="ABD_DATA=$cosmetic_stage/data $ABD"
    adb shell "$fixture_abd app add $PACKAGE"
    adb shell "test ! -e $cosmetic_stage/data/settings" || die "the fixture should start without saved settings"
    adb shell "cat > $cosmetic_stage/daemon" <<'ANDROID'
set -eu
mount -o rprivate none /
mount -o bind "$1/system-roots" /apex/com.android.conscrypt/cacerts
export ABD_DATA="$1/data"
umask 077
printf '%s\n' "$$" > "$ABD_DATA/abd.pid"
exec "$2" run
ANDROID
    cosmetic_daemon_replaced=1
    adb shell "sh $MODPATH/stop.sh"
    adb shell "timeout 55 unshare -m sh $cosmetic_stage/daemon $cosmetic_stage $ABD" >"$work/cosmetic-daemon.log" 2>&1 &
    await_cosmetic_capture() {
        local expected=$1 i
        for ((i=0; i<50; i++)); do
            if adb shell "$fixture_abd status --json" |
                jq -e --argjson expected "$expected" '.running and .rules != null and .filtering_apps == $expected' >/dev/null &&
                adb exec-out cat "$cosmetic_stage/data/state" 2>/dev/null | jq -e '.exclusions_ready == true' >/dev/null; then
                return 0
            fi
            sleep .2
        done
        return 1
    }
    await_cosmetic_capture 1 || die "the fixture daemon did not select the WebView app"
    adb shell "$fixture_abd status --json" | jq -e '.cosmetic == true' >/dev/null ||
        die "the fixture daemon did not enable element hiding by default"

    local result_file="/data/user/0/$PACKAGE/files/result" expected result cosmetic
    for cosmetic in true false; do
        if [[ $cosmetic == true ]]; then
            expected='[42,false,true,true]'
        else
            expected='[null,false,false,true]'
            # The status flag reads the saved setting. Re-entry into
            # capture confirms the daemon has applied the change.
            adb shell "$fixture_abd app pause $PACKAGE --minutes 5"
            await_cosmetic_capture 0 || die "the fixture daemon did not pause the WebView app"
            adb shell "$fixture_abd cosmetic off && $fixture_abd app resume $PACKAGE"
            await_cosmetic_capture 1 || die "the fixture daemon did not resume the WebView app after disabling element hiding"
        fi
        adb shell "rm -f $result_file; am start --user 0 -S -W -n $PACKAGE/.Main \
            --es url 'https://fixture.example/?cosmetic=$cosmetic' --ez webview true --ez expectCosmetic $cosmetic" >/dev/null
        result=
        for ((i=0; i<75; i++)); do
            result=$(adb shell "cat $result_file 2>/dev/null" | tr -d '\r' || true)
            [[ -z $result ]] || break
            sleep .2
        done
        [[ $result == "$expected" ]] || die "WebView with cosmetic=$cosmetic: wanted $expected, got ${result:-no result}"
        echo "ok    local HTTPS WebView cosmetic=$cosmetic ($result)"
    done
}
