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

# Checks on a device that the installed module filters an app's traffic.
#
#   scripts/check-device.sh
#
# Builds a small app (scripts/check/app), installs it, has the module filter
# it and has it fetch a few addresses, as an app does: with the platform's
# HTTP client, trusting what the platform has it trust. For one host the app
# trusts only roots the user added, as a browser built on Chromium goes by.
#
# Needs adb as root (an emulator with a google_apis image, after
# install-on-emulator.sh), a JDK, and the Android build tools and a platform
# in the SDK.
set -euo pipefail

cd "$(dirname "$0")/.."

PACKAGE=li.mercury.adblocker.check
case "${1:-}" in
    "") ;;
    --cosmetic) ;;
    *) echo "usage: $0 [--cosmetic]" >&2; exit 2 ;;
esac
DATA=/data/adb/adblocker
LIST=$DATA/rules/zz-check.txt
EXCLUDED=$DATA/exclusions/zz-check.txt
# The capture and bypass marks in abd/src/capture.rs.
CAPTURE=0x4000000
BYPASS=0x8000000

die() {
    echo "error: $*" >&2
    exit 1
}

# Inspect the rules this checkout builds. The daemon owns the kernel names.
rule_name() {
    local name
    name=$(sed -n "s/^const $1: &str = \"\([a-z0-9_]*\)\";$/\1/p" abd/src/rules.rs)
    [[ $name =~ ^[a-z][a-z0-9_]*$ ]] || die "could not read $1 from abd/src/rules.rs"
    printf '%s\n' "$name"
}
selector_chain=$(rule_name OUT)
mark_chain=$(rule_name MARK)
capture_quota=$(rule_name GATE_NAME)
quic_chain=$(rule_name QUIC)
proxy_chain=$(rule_name PRE)

# With more than one device attached, adb would refuse every command, or the
# script would act on a device it was not meant for: ANDROID_SERIAL,
# which adb reads, names the one.
if [[ -z "${ANDROID_SERIAL:-}" ]]; then
    attached="$(adb devices | awk 'NR > 1 && NF >= 2 { print $1 }')"
    count="$(printf '%s\n' "$attached" | awk 'NF { n++ } END { print n + 0 }')"
    if ((count > 1)); then
        echo "error: $count devices are attached; set ANDROID_SERIAL to one of them:" >&2
        printf '  %s\n' $attached >&2
        exit 2
    fi
fi
# The module in a root manager once it has its daemon, or else the one
# installed without a manager (module/direct.sh).
MODPATH=$(adb shell 'm=/data/adb/modules/adblocker; [ -e $m/bin/abd ] || m=/data/adb/adblocker_direct; echo $m' | tr -d '\r')
[[ $MODPATH == /data/adb/* ]] || die "could not ask the device where the module is"
ABD=$MODPATH/bin/abd

ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
tools="$(find "$ANDROID_HOME/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)"
platform="$(find "$ANDROID_HOME/platforms" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)/android.jar"
[[ -x "$tools/aapt2" && -f "$platform" ]] || die "build tools or platform not found in $ANDROID_HOME"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
uid=0
scope_fixture=
profile_id=
work_key=
lock_dir=
lock_holder=
release_lock() {
    if [[ -n $lock_dir ]]; then
        adb shell "touch $lock_dir/release" || true
    fi
    if [[ -n $lock_holder ]]; then
        wait "$lock_holder" || true
        lock_holder=
    fi
}
cleanup() {
    if [[ -n ${dns_held:-} ]]; then adb shell "kill -CONT $dns_held" || true; fi
    if [[ -n ${dns_fixture:-} ]]; then adb shell "pm uninstall $dns_fixture" >/dev/null 2>&1 || true; fi
    if [[ -n ${vpn_trace_job:-} ]]; then stop_vpn_trace; fi
    release_lock
    if [[ -n $work_key ]]; then
        adb shell "$ABD app remove $work_key" >/dev/null 2>&1 || true
    fi
    if [[ -n $profile_id ]]; then
        if [[ ${work_vpn_active:-0} == 1 ]] && ! clear_work_vpn; then
            echo "error: could not clear test VPN lockdown; retained disposable user $profile_id" >&2
        else
            adb shell pm remove-user "$profile_id" >/dev/null 2>&1 || true
        fi
    fi
    if [[ -n $lock_dir ]]; then
        adb shell "rm -r $lock_dir" >/dev/null 2>&1 || true
    fi
    if [[ -n $scope_fixture ]]; then
        adb shell "$ABD app include $scope_fixture; pm uninstall $scope_fixture" >/dev/null 2>&1 || true
    fi
    case ${original_settings:-} in
        present) adb shell "umask 077; cat >$DATA/settings.check && mv $DATA/settings.check $DATA/settings" <"$work/settings" ;;
        absent) adb shell "rm -f $DATA/settings" ;;
    esac
    adb shell "$ABD reload" >/dev/null 2>&1 || true
    rm -rf "$work"
    adb shell "rm -f $LIST $EXCLUDED; $ABD app remove $PACKAGE; pm uninstall $PACKAGE
        iptables -w -D OUTPUT -m owner --uid-owner $uid -m mark --mark $BYPASS/$BYPASS" >/dev/null 2>&1 || true
}
# ---- the app -----------------------------------------------------------------
# The administrator provisions trust. A distinct, ordinary app exercises
# the traffic path, including Android's normal force-stop/start semantics.
# Android protects a profile owner from being force-stopped.
keytool -genkeypair -keystore "$work/key.jks" -storepass android -keypass android \
    -alias check -keyalg RSA -keysize 2048 -validity 2 -dname CN=check >/dev/null 2>&1
build_check_apk() {
    local src=$1 name=$2 stage="$work/$2"
    shift 2
    mkdir -p "$stage/classes"
    "$tools/aapt2" compile --dir "$src/res" -o "$stage/res.zip"
    "$tools/aapt2" link -o "$stage/app.apk" -I "$platform" --manifest "$src/AndroidManifest.xml" "$@" "$stage/res.zip"
    find "$src/java" -name '*.java' -exec javac --release 11 -Xlint:-options -cp "$platform" -d "$stage/classes" {} +
    find "$stage/classes" -name '*.class' -exec "$tools/d8" --lib "$platform" --output "$stage" {} +
    (cd "$stage" && zip -q -j app.apk classes.dex)
    "$tools/zipalign" -f 4 "$stage/app.apk" "$stage/aligned.apk"
    "$tools/apksigner" sign --ks "$work/key.jks" --ks-pass pass:android --out "$work/$name.apk" "$stage/aligned.apk"
}
if [[ ${1:-} == --cosmetic ]]; then
    # The cosmetic fixture owns its daemon, app and network until it exits.
    source scripts/check/cosmetic.sh
    check_cosmetic
    exit
fi

trap cleanup EXIT
# Cleanup removes or restores the file only once it is known which applies.
settings_state=$(adb shell "test -f $DATA/settings && echo present || echo absent")
if [[ $settings_state == present ]]; then
    adb exec-out cat "$DATA/settings" >"$work/settings"
fi
original_settings=$settings_state

build_check_apk scripts/check/app check
adb install -r "$work/check.apk" >/dev/null 2>&1

uid="$(adb shell "pm list packages -U $PACKAGE" | tr -d '\r' | sed -n "s/^package:$PACKAGE uid://p")"
[[ $uid =~ ^[0-9]+$ ]] || die "the check app did not install"

# An app-triggered update can prune the check's temporary exclusion list.
# Wait for any running update before writing the fixture.
for ((i = 0; i < 180; i++)); do
    busy="$(adb shell "if pgrep -f 'bin/ab[d] update' >/dev/null; then echo updating; fi" | tr -d '\r')"
    [[ -z $busy ]] && break
    sleep 1
done
[[ -z $busy ]] || die "a list update is still running"

# A fresh module only filters domains until all exclusions have downloaded.
# Conscrypt and the legacy system directory may ship different root sets.
# Compare each actual global store to itself before this app is selected.
global_stores() {
    adb shell 'for dir in /apex/com.android.conscrypt/cacerts /system/etc/security/cacerts /data/misc/user/0/cacerts-added; do
        if [ -d "$dir" ]; then sha256sum "$dir"/* 2>/dev/null; fi
    done | sort'
}
stores_before=$(global_stores)
[[ -n $stores_before ]] || die "could not read the certificate store baseline"
# Start from the defaults: the decryption checked below needs no setting.
# The cleanup puts the original settings back.
adb shell "rm -f $DATA/settings && $ABD scope chosen && $ABD app add $PACKAGE"
if adb shell "$ABD app list --json" | grep -F '"reason":"exclusions_pending"' >/dev/null; then
    adb shell "$ABD update --json" > "$work/update.jsonl"
    grep -F '"exclusions_ready":true' "$work/update.jsonl" >/dev/null || die "HTTPS exclusions could not be downloaded"
fi

# Rules of the check's own, so that it does not depend on what the
# subscribed lists hold today.
adb shell "umask 077; mkdir -p $DATA/rules && cat > $LIST" <<'RULES'
||example.com/blocked-by-check
||www.wikipedia.org/blocked-by-check
||example.org^
@@|https://example.org/|
|http://192.0.2.17/abd-managed-blocked|
RULES
adb shell "umask 077; mkdir -p $DATA/exclusions && echo www.iana.org > $EXCLUDED"
# Counts the packets of the daemon's connections that the packet filter
# takes for the app's: what any other module's rules for the app go by.
adb shell "iptables -w -I OUTPUT -m owner --uid-owner $uid -m mark --mark $BYPASS/$BYPASS"
adb shell "$ABD reload"
# The daemon reads its lists and apps again when told; give it a moment.
sleep 3
# The counts persist across daemons and runs of this check: what this run
# did is the difference from now.
counts_before="$(adb shell "$ABD stats" | tr -d '\r')"
count() { sed -n "s/^$1 //p" <<<"$2"; }

# What the app says of an address: started anew for each, so that each is a
# process that has yet to be made to trust the authority.
fetch() {
    local user=${2:-0} body=${3:-false} client=${4:-$PACKAGE} udp=${5:-false} result_file result='' i
    result_file="/data/user/$user/$client/files/result"
    adb shell "rm -f $result_file; am start --user $user -S -W -n $client/$PACKAGE.Main --es url '$1' --ez body $body --ez udp $udp" >/dev/null 2>&1
    for ((i = 0; i < 40; i++)); do
        result="$(adb shell "cat $result_file 2>/dev/null" | tr -d '\r' || true)"
        [[ -n $result ]] && break
        sleep 0.5
    done
    echo "${result:-no answer}"
}

# What an app is told of a host: by the system resolver, or by the
# network's DNS server asked directly with `--es dns udp|tcp`. Or, given
# other extras such as `--ei delayMs`, what it fetched.
look_up() {
    local name=$1 client=${2:-$PACKAGE} how=${3:---ez resolve true} result_file result='' i
    result_file="/data/user/0/$client/files/result"
    adb shell "rm -f $result_file; am start --user 0 -S -W -n $client/$PACKAGE.Main --es url '$name' $how" >/dev/null 2>&1
    for ((i = 0; i < 40; i++)); do
        result="$(adb shell "cat $result_file 2>/dev/null" | tr -d '\r' || true)"
        [[ -n $result ]] && break
        sleep 0.5
    done
    echo "${result:-no answer}"
}

# The time the daemon took the host for pinned for the check app, if it does.
pinned_at() {
    adb shell "$ABD pinned" | tr -d '\r' | awk -v p="$PACKAGE" -v h="$1" '$1 == p && $2 == h { print $3 }'
}

# Fetches the address in a new process of the check app, connecting only
# once the daemon has given that process its copies of the stores, as it
# does when a process starts, and with them the try of the hosts its app
# pins.
fetch_when_mounted() {
    local address=$1 files="/data/user/0/$PACKAGE/files" pid result='' i
    adb shell "rm -f $files/result $files/probe-ready $files/probe-go; am start --user 0 -S -W -n $PACKAGE/$PACKAGE.Main --es url '$address' --ez waitForGo true" >/dev/null 2>&1
    for ((i = 0; i < 40; i++)); do
        pid=$(adb shell "pidof $PACKAGE" | tr -d '\r')
        if [[ -n $pid ]] && adb shell "test -f $files/probe-ready && grep -q ' /adb/adblocker/store ' /proc/${pid%% *}/mountinfo"; then
            break
        fi
        sleep 0.5
    done
    ((i < 40)) || {
        echo "no mounts in the new process"
        return
    }
    # The try is given just after the mounts are made.
    sleep 1
    adb shell "touch $files/probe-go"
    for ((i = 0; i < 40; i++)); do
        result="$(adb shell "cat $files/result 2>/dev/null" | tr -d '\r' || true)"
        [[ -n $result ]] && break
        sleep 0.5
    done
    echo "${result:-no answer}"
}

failed=0
expect() {
    local what="$1" want="$2" got="$3"
    if [[ $got == $want ]]; then
        echo "ok    $what"
    else
        echo "FAIL  $what: got '$got', wanted '$want'"
        failed=1
    fi
}

default_status="$(adb shell "$ABD status --json" | tr -d '\r')"
expect "URL filtering is on by default" '*"url_filtering":true*' "$default_status"
expect "page element hiding is on by default" '*"cosmetic":true*' "$default_status"
expect "a request is decrypted and let through, trusting the system store" "200 *ADBlocker*" "$(fetch https://example.com/)"
expect "a request is decrypted, trusting what the user added" "200 *ADBlocker*" "$(fetch https://www.wikipedia.org/)"
expect "a request is blocked by its path" "403 *ADBlocker*" "$(fetch https://example.com/blocked-by-check)"
# A full-mode connection can be allowed through to evaluate a path exception.
# Turn off URL filtering when checking the pre-TLS refusal.
adb shell "$ABD url-filtering off"
for ((attempt = 0; attempt < 20; attempt++)); do
    got=$(fetch https://example.org/)
    [[ $got == error* ]] && break
    sleep 1
done
expect "URL filtering off still refuses a host with a whole-host rule" "error *" "$got"
adb shell "$ABD url-filtering on"
for ((attempt = 0; attempt < 20; attempt++)); do
    got=$(fetch https://example.com/blocked-by-check)
    [[ $got == "403 "*ADBlocker* ]] && break
    sleep 1
done
expect "URL filtering resumes for the selected app" "403 *ADBlocker*" "$got"

got="$(fetch https://www.iana.org/)"
[[ $got == 200* && $got != *ADBlocker* ]] && got="the server's own certificate"
expect "a host a list excludes is not decrypted" "the server's own certificate" "$got"
packets="$(adb shell "iptables -w -L OUTPUT -v -n -x | grep -m 1 'owner UID match $uid mark match' | awk '{print \$1}'" | tr -d '\r')"
expect "the daemon's connections are the app's own to the packet filter" "[1-9]*" "${packets:-none}"

expect "a cleartext request is blocked by path" "403 -" "$(fetch http://example.com/blocked-by-check)"
expect "a cleartext request to the same host is allowed" "[23]0? -" "$(fetch http://example.com/)"

# Nobody but the filtered app is made to trust the authority: not the
# system, whose stores are as they were, and not another app's process.
if [[ $(global_stores) == "$stores_before" ]]; then stores=unchanged; else stores=changed; fi
expect "the global and personal certificate stores keep their contents" "unchanged" "$stores"
other="$(adb shell "pidof com.android.settings || (am start -n com.android.settings/.Settings >/dev/null 2>&1; sleep 2; pidof com.android.settings)" | tr -d '\r' | awk '{print $1}')"
expect "another app's process has no mount of ours" "0" "$(adb shell "grep -c adblocker /proc/$other/mountinfo" | tr -d '\r')"
# After a connection of its own that was decrypted: the stores are mounted
# when one first is, and this is a new process.
fetch https://example.com/ >/dev/null
mine="$(adb shell "pidof $PACKAGE" | tr -d '\r' | awk '{print $1}')"
expect "the filtered app's process has both, read-only" "2" "$(adb shell "grep adblocker /proc/$mine/mountinfo | grep -c ' ro,'" | tr -d '\r')"

# An app that connects to the daemon's own port, rather than being brought
# there by the rules, is refused: carried, the daemon would connect to
# itself without end.
# The connections' listener is the one TPROXY delivers them to: the daemon
# listens for lookups on another port.
port="$(adb shell "iptables -w 5 -t mangle -S $proxy_chain" | tr -d '\r' | sed -n 's/.*--on-port \([0-9]*\).*/\1/p' | head -n 1)"
[[ $port =~ ^[0-9]+$ ]] || die "the daemon is not listening"
expect "a direct connection from the check app is refused" "error *" "$(fetch "http://127.0.0.1:$port/")"
expect "the daemon is still there after being connected to directly" "200 *ADBlocker*" "$(fetch https://example.com/)"

# Counts are written some time after they change.
stats="$(adb shell "sleep 31; $ABD stats" | tr -d '\r')"
for counter in decrypted requests_allowed requests_blocked hosts_blocked passed refused; do
    was="$(count "$counter" "$counts_before")"
    now="$(count "$counter" "$stats")"
    if [[ -n $now ]] && ((now > ${was:-0})); then grew=yes; else grew="${now:-missing}, was ${was:-0}"; fi
    expect "$counter is counted" "yes" "$grew"
done
# Refusing the app's first connections for want of trust would have its
# hosts taken for pinned.
pinned_before="$(count pinned "$counts_before")"
expect "no host was taken for pinned" "${pinned_before:-0}" "$(count pinned "$stats")"

# Read the actual kernel selector, which may combine consecutive chosen UIDs.
selected_uid() {
    local target=$1 family=$2 tool=iptables rules
    if [[ $family == 6 ]]; then tool=ip6tables; fi
    rules=$(adb shell "$tool -w 1 -t mangle -S $selector_chain" 2>&1) || {
        if [[ $rules == *'No chain/target/match by that name'* ]]; then return 1; fi
        return 2
    }
    awk -v uid="$target" -v chain="$selector_chain" -v mark="$mark_chain" '
        $1 == "-A" && $2 == chain && $(NF-1) == "-g" && $NF == mark {
            for (i = 1; i < NF; i++) if ($i == "--uid-owner") {
                n = split($(i + 1), bounds, "-")
                if (uid >= bounds[1] && uid <= bounds[n]) found = 1
            }
        }
        END { exit !found }
    ' <<<"$rules"
}

await_selected_uid() {
    local target=$1 expected=$2 family actual attempt
    for ((attempt = 0; attempt < 30; attempt++)); do
        for family in "${capture_families[@]}"; do
            actual=0
            selected_uid "$target" "$family" || actual=$?
            [[ $actual == "$expected" ]] || break
        done
        if [[ $actual == "$expected" ]]; then
            if [[ $expected == 0 ]]; then
                for family in "${capture_families[@]}"; do await_gate "$family" 0; done
                await_quota 2
            fi
            return 0
        fi
        sleep 1
    done
    die "IPv$family kernel selection for fixture UID $target returned $actual, expected $expected"
}

gate_for() {
    local family=$1 rules
    rules=$(adb shell "ip -$family rule show") || return 2
    [[ $rules == *"fwmark $CAPTURE/$CAPTURE lookup "* ]]
}

await_gate() {
    local family=$1 expected=$2 limit=${3:-30} actual attempt
    for ((attempt = 0; attempt < limit; attempt++)); do
        actual=0
        gate_for "$family" || actual=$?
        [[ $actual == "$expected" ]] && return 0
        sleep 1
    done
    die "IPv$family capture routing gate returned $actual, expected $expected"
}

await_quota() {
    local expected=$1 value attempt
    for ((attempt = 0; attempt < 30; attempt++)); do
        value=$(adb shell "cat /proc/net/xt_quota/$capture_quota 2>/dev/null" || true)
        [[ $value == "$expected" ]] && return 0
        sleep 1
    done
    die "the capture quota is '$value', expected '$expected'"
}

assert_xtables_locked() {
    local result
    result=$(adb shell 'iptables -w 1 -S' 2>&1 || true)
    [[ $result == *'holding the xtables lock'* ]] || die "the xtables lock fixture is not holding the lock"
}

hold_xtables_lock() {
    local stage attempt
    if [[ -z $lock_dir ]]; then
        stage=$(adb shell 'umask 077; mktemp -d /data/local/tmp/abd-xtables.XXXXXX' | tr -d '\r')
        [[ $stage =~ ^/data/local/tmp/abd-xtables\.[[:alnum:]]+$ ]] || die "could not create the lock fixture"
        lock_dir="$stage"
    fi
    adb shell "rm -f $lock_dir/ready $lock_dir/release"

    # AOSP iptables uses this file unless XTABLES_LOCKFILE is set.
    adb shell "timeout 60 sh -s '$lock_dir'" >"$work/xtables.log" 2>&1 <<'LOCK' &
set -eu
work=$1
{
    # Toybox flock takes a descriptor. The shell keeps stdin open for the group.
    flock -x 0
    touch "$work/ready"
    while [ ! -e "$work/release" ]; do sleep 0.2; done
} </system/etc/xtables.lock
LOCK
    lock_holder=$!
    for ((attempt = 0; attempt < 15; attempt++)); do
        adb shell "test -f $lock_dir/ready" && break
        kill -0 "$lock_holder" 2>/dev/null || break
        sleep 1
    done
    if ! adb shell "test -f $lock_dir/ready"; then
        cat "$work/xtables.log" >&2
        die "the xtables lock fixture did not start"
    fi
    assert_xtables_locked
}

crash_under_lock() {
    local before_kill=${1:-:} status
    adb shell "iptables -w 2 -t filter -S $quic_chain" |
        grep -- "--name $capture_quota .* -j REJECT" >/dev/null ||
        die "UDP/443 rejection does not use the shared capture quota"
    await_gate 4 0
    await_quota 2
    check_v6=0
    status=$(adb shell "$ABD status --json")
    if [[ $status == *'"ipv6":true'* ]]; then
        await_gate 6 0
        check_v6=1
    fi
    hold_xtables_lock
    "$before_kill"

    adb shell "kill -9 \$(cat $DATA/abd.pid)"
    await_quota 0
    await_gate 4 1
    if [[ $check_v6 == 1 ]]; then await_gate 6 1; fi
    assert_xtables_locked
    echo "ok    the supervisor closes the capture quota and removes both routing gates while xtables is locked"
}

release_crash_lock() {
    assert_xtables_locked
    release_lock
    # The supervisor backs off after crashes. Wait for the kernel gate before
    # sending any request that is expected to be filtered by the replacement.
    await_gate 4 0 90
    await_quota 2
    if [[ $check_v6 == 1 ]]; then await_gate 6 0; fi
}

status=$(adb shell "$ABD status --json")
capture_families=(4)
if [[ $status == *'"ipv6":true'* ]]; then capture_families+=(6); fi

source scripts/check/dns.sh
check_dns
source scripts/check/pins.sh
check_pin_retry
source scripts/check/pin-hook.sh
check_pin_hook

# Install test packages and create a disposable profile only on the test emulator.
if [[ $(adb shell getprop ro.kernel.qemu | tr -d '\r') == 1 ]]; then
    # Use a package the daemon has never seen, installed after the scope changed.
    scope_package=$PACKAGE.scope
    if adb shell pm list packages --user 0 "$scope_package" | tr -d '\r' | grep -Fx "package:$scope_package"; then
        die "the All apps fixture package already exists"
    fi
    scope_fixture=$scope_package
    build_check_apk scripts/check/app scope --rename-manifest-package "$scope_package"
    adb shell "$ABD url-filtering off && $ABD scope all"
    adb install --user 0 "$work/scope.apk" >/dev/null
    scope_uid=$(adb shell pm list packages --user 0 -U "$scope_package" | tr -d '\r' | sed -n "s/^package:$scope_package uid://p")
    [[ $scope_uid =~ ^[0-9]+$ ]] && ((scope_uid >= 10000 && scope_uid <= 19999)) || die "the All apps fixture has no app UID"
    await_selected_uid "$scope_uid" 0
    await_selected_uid 1000 1
    await_selected_uid 2000 1
    await_selected_uid 0 1
    if adb shell "$ABD app selections --json" | jq -e --arg app "$scope_package" '.chosen | index($app) != null' >/dev/null; then
        die "the All apps fixture should never have been chosen"
    fi
    expect "All apps filters a newly installed, never chosen app" "error *" "$(fetch https://example.org/ 0 false "$scope_package")"
    expect "a system UID reaches the same domain without filtering" "HTTP/*" "$(
        adb shell "{ printf 'GET / HTTP/1.1\r\nHost: example.org\r\nConnection: close\r\n\r\n'; sleep 2; } | nc -w 8 -W 5 example.org 80" | awk 'NR == 1'
    )"
    adb shell "$ABD app pause $scope_package --minutes 1"
    await_selected_uid "$scope_uid" 1
    expect "pausing an implicit app bypasses filtering" "200 *" "$(fetch https://example.org/ 0 false "$scope_package")"
    adb shell "$ABD app resume $scope_package"
    await_selected_uid "$scope_uid" 0
    adb shell "$ABD app exclude $scope_package"
    await_selected_uid "$scope_uid" 1
    expect "All apps leaves an excluded app unfiltered" "200 *" "$(fetch https://example.org/ 0 false "$scope_package")"
    adb shell "$ABD app include $scope_package"
    await_selected_uid "$scope_uid" 0
    expect "including an excluded app restores filtering" "error *" "$(fetch https://example.org/ 0 false "$scope_package")"
    adb shell "$ABD scope chosen && $ABD url-filtering on"
    await_selected_uid "$scope_uid" 1
    adb uninstall "$scope_package" >/dev/null
    scope_fixture=
    source scripts/check/shared-uid.sh
    source scripts/check/work-profile.sh
else
    crash_under_lock
    expect "the app can reach a previously blocked path while the tables are locked" "404 *" "$(fetch https://example.com/blocked-by-check)"
    release_crash_lock
    await_selected_uid "$uid" 0
    expect "a new daemon filters again" "403 *ADBlocker*" "$(fetch https://example.com/blocked-by-check)"
fi
exit "$failed"
