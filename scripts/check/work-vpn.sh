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

# Sourced from work-profile.sh after the test app and profile owner are installed.
vpn_action() {
    adb shell am start --user "$profile_id" -W -n "$ADMIN/.VpnCheckConfig" --es action "$1" >/dev/null
}

await_vpn_file() {
    local file=$1 expected=$2 actual attempt
    for ((attempt = 0; attempt < 30; attempt++)); do
        actual=$(adb shell "cat $vpn_files/$file 2>/dev/null" || true)
        [[ $actual == "$expected" ]] && return
        sleep 1
    done
    echo "error: managed VPN $file was '$actual', expected '$expected'" >&2
    return 1
}

await_vpn_network() {
    local expected=$1 connected attempt state
    for ((attempt = 0; attempt < 30; attempt++)); do
        state=$(adb shell dumpsys connectivity) || return 1
        connected=no
        # Android 14 can leave the VPN extra empty. Match ownership on the
        # connected network itself so a personal VPN cannot satisfy the check.
        if awk -v uid="$vpn_owner_uid" '/NetworkAgentInfo/ && /VPN CONNECTED/ {
            for (i = 1; i < NF; i++)
                if ($i == "OwnerUid:" && $(i + 1) == uid) found = 1
        } END { exit !found }' <<< "$state"; then
            connected=yes
        fi
        if [[ $connected == "$expected" ]]; then
            echo "ok    Android reports the managed VPN network connected=$connected"
            return
        fi
        sleep 1
    done
    echo "error: Android reports the managed VPN network connected=$connected, expected $expected" >&2
    return 1
}

clear_work_vpn() {
    vpn_action clear && await_vpn_file vpn-policy "null false" && await_vpn_network no || return 1
    work_vpn_active=0
}

fetch_vpn_udp() {
    fetch "$vpn_target" "$profile_id" false "${1:-$PACKAGE}" true
}

# Android 15's packet capture can identify the device interface. Trace only
# traffic addressed to the synthetic test origin.
start_vpn_trace() {
    local attempt
    if ! adb shell 'test -x /system/bin/tcpdump && tcpdump -i any -L 2>&1 | grep -qw LINUX_SLL2'; then
        echo "skip  the test emulator lacks tcpdump with interface names"
        return
    fi
    adb shell "timeout 45 sh -s '$lock_dir/vpn-trace.pid'" >"$work/vpn-trace.log" 2>&1 <<'TRACE' &
set -eu
printf '%s\n' "$$" > "$1"
exec tcpdump -p -n -l -q -i any -y LINUX_SLL2 -Q out \
    'dst host 192.0.2.17 and (tcp dst port 80 or udp dst port 443)'
TRACE
    vpn_trace_job=$!
    for ((attempt = 0; attempt < 20; attempt++)); do
        grep -Fq 'listening on any' "$work/vpn-trace.log" && return 0
        kill -0 "$vpn_trace_job" 2>/dev/null || break
        sleep 0.2
    done
    cat "$work/vpn-trace.log" >&2
    die "could not start the synthetic traffic trace"
}

stop_vpn_trace() {
    adb shell "pid=\$(cat $lock_dir/vpn-trace.pid 2>/dev/null); if [ -n \"\$pid\" ]; then
        case \"\$(ps -p \"\$pid\" -o ARGS=)\" in
            *'tcpdump '*192.0.2.17*) kill -INT \"\$pid\" ;;
        esac
    fi" >/dev/null 2>&1 || true
    wait "$vpn_trace_job" || true
    vpn_trace_job=
}

check_vpn_trace() {
    [[ -n ${vpn_trace_job:-} ]] || return 0
    stop_vpn_trace
    local phase=$1 leaked tcp udp
    read -r leaked tcp udp < <(awk '
        $3 == "Out" && $4 == "IP" {
            if ($2 ~ /^tun[0-9]+$/) {
                if ($0 ~ /> 192\.0\.2\.17\.80:/) tcp++
                if ($0 ~ /> 192\.0\.2\.17\.443:/) udp++
            } else if ($2 != "lo") leaked++
        }
        END { print leaked + 0, tcp + 0, udp + 0 }
    ' "$work/vpn-trace.log")
    if ! grep -Fq '0 packets dropped by kernel' "$work/vpn-trace.log"; then
        cat "$work/vpn-trace.log" >&2
        die "the synthetic traffic trace did not complete without kernel drops"
    fi
    if [[ $phase == disconnected ]]; then
        expect "no synthetic work UDP leaves a physical interface after tunnel disconnect" 0 "$leaked"
        if ((leaked != 0)); then cat "$work/vpn-trace.log"; fi
        return
    fi
    expect "no synthetic work traffic exits a physical interface during the crash" 0 "$leaked"
    expect "the managed TUN carries TCP during the crash" '[1-9]*' "$tcp"
    if ((udp >= 2)); then
        echo "ok    the managed TUN carries both work clients' UDP probes during the crash"
    else
        expect "the managed TUN carries both work clients' UDP probes during the crash" "at least 2" "$udp"
    fi
    if ((leaked != 0 || tcp == 0 || udp < 2)); then cat "$work/vpn-trace.log"; fi
}

pending_selector_restore() {
    local daemon=$1
    adb shell ps -A -o PID,PPID,ARGS | awk -v daemon="$daemon" '
        $2 == daemon && /\/system\/bin\/iptables-restore[[:space:]]/ { print $1; exit }
    '
}

pause_personal_while_work_is_retained() {
    local files="/data/user/$profile_id/$PACKAGE/files"
    local during daemon restore after quota result=0 attempt
    # Start the app before holding xtables. The numeric TCP and UDP probes
    # run during the restore, without changing the app or network state.
    adb shell "rm -f $files/probe-ready $files/probe-go $files/result;
        am start --user $profile_id -S -W -n $PACKAGE/.Main \
          --es url '$vpn_target/abd-managed-blocked' --ez waitForProbe true" >/dev/null
    adb shell "timeout 4 sh -c 'until test -s $files/probe-ready; do sleep 0.05; done'"
    daemon=$(adb shell "cat $DATA/abd.pid")
    [[ $daemon =~ ^[1-9][0-9]*$ ]] || die "no running daemon to check"
    hold_xtables_lock
    adb shell "$ABD app pause $PACKAGE --minutes 5"
    for ((attempt = 0; attempt < 20; attempt++)); do
        restore=$(pending_selector_restore "$daemon")
        [[ -n $restore ]] && break
        sleep 0.1
    done
    [[ -n $restore ]] || die "the healthy selector restore never reached the held xtables lock"
    quota=$(adb shell "cat /proc/net/xt_quota/$capture_quota")
    gate_for 4 || result=$?
    during=$(adb shell "touch $files/probe-go;
        timeout 2 sh -c 'until test -s $files/result; do sleep 0.05; done; cat $files/result'") || during="no answer during restore"
    after=$(pending_selector_restore "$daemon")
    release_lock
    [[ $after == "$restore" ]] || die "the healthy selector restore did not stay pending for the work request"
    expect "capture stays active while the healthy selector waits on xtables" "2 0" "$quota $result"
    expect "work TCP and UDP/443 stay filtered while the personal pause waits" "403 - / error PortUnreachableException" "$during"
    await_selected_uid "$uid" 1
    await_selected_uid "$work_uid" 0
    expect "the work rule remains active after the personal pause is applied" "403 -" "$(fetch "$vpn_target/abd-managed-blocked" "$profile_id")"
    adb shell "$ABD app resume $PACKAGE"
    await_selected_uid "$uid" 0
}

vpn_files="/data/user/$profile_id/$ADMIN/files"
# These body comparisons use cleartext HTTP, which is ineligible for page element hiding.
vpn_target=http://192.0.2.17
# Use the same routable address without asking DNS while lockdown is active.
direct_ip=$(adb shell 'ping -c 1 -W 1 example.com 2>&1 || true' |
    sed -n 's/^PING [^(]*(\([0-9.]*\)).*/\1/p')
[[ $direct_ip =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "could not resolve the direct-path test address"
direct_url="http://$direct_ip/"
direct_reply=$(fetch "$direct_url" "$profile_id" true)
[[ $direct_reply == [234][0-9][0-9]' - '?* ]] || die "the IPv4 literal did not return a direct HTTP body: $direct_reply"
echo "ok    the work copy reaches $direct_ip before lockdown ($direct_reply)"
expect "the working direct path is available before enabling work VPN lockdown" "[23]0? -" "$(fetch http://example.com/ "$profile_id")"

# Keep the work app selected while the VPN starts. A second work UID
# checks that Android can also route an unfiltered app through the tunnel.
vpn_control="$PACKAGE.control"
mkdir "$work/vpn-control-src"
cp -R scripts/check/app/. "$work/vpn-control-src/"
sed 's/package="li.mercury.adblocker.check"/package="li.mercury.adblocker.check.control"/;
     s/android:name="[.]Main"/android:name="li.mercury.adblocker.check.Main"/' \
    scripts/check/app/AndroidManifest.xml > "$work/vpn-control-src/AndroidManifest.xml"
build_check_apk "$work/vpn-control-src" vpn-control
adb install --user "$profile_id" -r "$work/vpn-control.apk" >/dev/null
control_uid=$(adb shell pm list packages --user "$profile_id" -U "$vpn_control" |
    awk -v package="package:$vpn_control" '$1 == package { sub(/^uid:/, "", $2); print $2 }')
[[ $control_uid =~ ^[0-9]+$ && $control_uid != "$work_uid" ]] || die "could not distinguish the two work UIDs"
await_selected_uid "$control_uid" 1
await_selected_uid "$work_uid" 0

# A working tunnel requires netd to update Android's iptables tables while
# filtering is already running. Keep the first connect under observation.
vpn_started=$(adb shell "date '+%m-%d %H:%M:%S.000'")
vpn_owner_uid=$(adb shell pm list packages --user "$profile_id" -U "$ADMIN" |
    awk -v package="package:$ADMIN" '$1 == package { sub(/^uid:/, "", $2); print $2 }')
[[ $vpn_owner_uid =~ ^[0-9]+$ ]] || die "could not resolve the managed VPN owner"
work_vpn_active=1
vpn_action connect
await_vpn_file vpn-policy "$ADMIN true"
await_vpn_file vpn-state connected
await_vpn_network yes
echo "ok    the work profile owner enabled its always-on VPN with lockdown"

adb shell "rm -f $vpn_files/vpn-request"
expect "an independent unselected work app confirms the managed TUN is usable" "200 - managed-tun-proof" "$(fetch "$vpn_target/abd-managed-allowed" "$profile_id" true "$vpn_control")"
expect "the unselected control reaches the managed TUN" "allowed" "$(adb shell "cat $vpn_files/vpn-request 2>/dev/null || true")"
await_selected_uid "$work_uid" 0
adb shell "rm -f $vpn_files/vpn-request"
expect "a selected work app gets the HTTP response served from the VPN TUN" "200 - managed-tun-proof" "$(fetch "$vpn_target/abd-managed-allowed" "$profile_id" true)"
expect "the selected app's allowed request reached the managed TUN" "allowed" "$(adb shell "cat $vpn_files/vpn-request 2>/dev/null || true")"
adb shell "rm -f $vpn_files/vpn-request"
expect "the daemon blocks a work request while its VPN is active" "403 -" "$(fetch "$vpn_target/abd-managed-blocked" "$profile_id")"
expect "the blocked request never reaches the VPN as HTTP" "absent" "$(adb shell "if test -f $vpn_files/vpn-request; then cat $vpn_files/vpn-request; else echo absent; fi")"
expect "the personal copy can connect while the work VPN is active" "200 *" "$(fetch https://example.com/)"
netd_denials=$(adb shell "logcat -b all -d -T '$vpn_started' -v brief" | awk '
    /avc: +denied/ && /prog_run/ && /scontext=u:r:netd:s0/ && /tclass=bpf/ { n++ }
    END { print n + 0 }
')
expect "netd updates its VPN rules without a BPF SELinux denial" 0 "$netd_denials"

expect "the unselected work app can exchange UDP/443 through the managed TUN" "managed-tun-udp" "$(fetch_vpn_udp "$vpn_control")"
expect "the selected work app receives a UDP/443 port unreachable" "error PortUnreachableException" "$(fetch_vpn_udp)"
pause_personal_while_work_is_retained
crash_under_lock start_vpn_trace
expect "a previously blocked work request reaches the managed TUN while xtables is locked" "200 - managed-tun-proof" "$(fetch "$vpn_target/abd-managed-blocked" "$profile_id" true)"
expect "the selected work app keeps its UDP/443 path while xtables is locked" "managed-tun-udp" "$(fetch_vpn_udp)"
expect "the unselected work app keeps its TCP path while xtables is locked" "200 - managed-tun-proof" "$(fetch "$vpn_target/abd-managed-allowed" "$profile_id" true "$vpn_control")"
expect "the unselected work app keeps its UDP/443 path while xtables is locked" "managed-tun-udp" "$(fetch_vpn_udp "$vpn_control")"
expect "the personal copy stays online while xtables is locked" "200 *" "$(fetch https://example.com/)"
check_vpn_trace crash
release_crash_lock
await_selected_uid "$work_uid" 0
expect "the replacement daemon filters the same work request" "403 -" "$(fetch "$vpn_target/abd-managed-blocked" "$profile_id")"
expect "the replacement daemon refuses selected UDP/443" "error PortUnreachableException" "$(fetch_vpn_udp)"

vpn_action disconnect
await_vpn_file vpn-policy "$ADMIN true"
await_vpn_file vpn-state disconnected
await_vpn_network no
start_vpn_trace
expect "disconnected work VPN lockdown prevents the selected UDP/443 probe" "error *" "$(fetch_vpn_udp)"
check_vpn_trace disconnected
expect "disconnected work VPN lockdown prevents direct connections" "error *" "$(fetch https://example.com/ "$profile_id")"
expect "disconnected work VPN lockdown also prevents a direct IPv4 connection" "error *" "$(fetch "$direct_url" "$profile_id" true)"
expect "disconnected work VPN lockdown leaves the personal copy online" "200 *" "$(fetch https://example.com/)"
expect "the personal copy still reaches that same IPv4 literal" "$direct_reply" "$(fetch "$direct_url" 0 true)"

clear_work_vpn
adb shell pm uninstall --user "$profile_id" "$vpn_control" >/dev/null
expect "the work copy can use the direct path again after its owner clears lockdown" "[23]0? -" "$(fetch http://example.com/ "$profile_id")"
expect "the same IPv4 direct path returns after lockdown is cleared" "$direct_reply" "$(fetch "$direct_url" "$profile_id" true)"
