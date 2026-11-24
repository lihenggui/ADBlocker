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

# Sourced by check-device.sh, with the check app selected: the app's plain
# lookups are filtered, nobody else's are, and a dead daemon or `abd
# unhook` leaves lookups as they were. The hosts are fresh names under a
# wildcard DNS service, so that no answer can come from Android's cache.

# The names lookup capture gives the kernel, from the source that owns them.
dns_name() {
    local name
    name=$(sed -n "s/^\(pub \)\{0,1\}const $1: [&a-z0-9']* = \"\{0,1\}\([a-z0-9_]*\)\"\{0,1\};$/\2/p" abd/src/dns/mod.rs)
    [[ $name =~ ^[a-z0-9_]+$ ]] || die "could not read $1 from abd/src/dns/mod.rs"
    printf '%s\n' "$name"
}
dns_table=$(dns_name TABLE)
dns_pref=$(dns_name PREF)
dns_device=$(dns_name DEVICE)
dns_chain=$(dns_name CHAIN)
# Every name under it is answered with the address in it.
dns_zone=192.0.2.1.nip.io
dns_real=192.0.2.1

root_look_up() {
    adb shell "ping -c 1 -W 1 $1 2>&1" | tr -d '\r' | sed -n 's/^PING [^ ]* (\([^)]*\)).*/\1/p'
}

# Whether a lookup rule of ours names the UID: 0 if one does, 1 if none.
dns_rule_for() {
    local rules
    rules=$(adb shell "ip rule show") || return 2
    awk -v uid="$1" -v pref="$dns_pref:" -v table="$dns_table" '
        $1 == pref && $NF == table {
            for (i = 2; i < NF; i++) if ($i == "uidrange") {
                split($(i + 1), bounds, "-")
                if (uid >= bounds[1] && uid <= bounds[2]) found = 1
            }
        }
        END { exit !found }
    ' <<<"$rules"
}

await_dns_rule() {
    local target=$1 expected=$2 limit=${3:-30} actual attempt
    for ((attempt = 0; attempt < limit; attempt++)); do
        actual=0
        dns_rule_for "$target" || actual=$?
        [[ $actual == "$expected" ]] && return 0
        sleep 1
    done
    die "the lookup rule for UID $target returned $actual, expected $expected"
}

# What is left of lookup capture in the kernel, one word for each part.
dns_leftovers() {
    adb shell "
        ip rule show | grep -q 'lookup $dns_table' && echo rule
        ip -6 rule show | grep -q 'lookup $dns_table' && echo rule6
        [ -n \"\$(ip route show table $dns_table)\" ] && echo route
        [ -n \"\$(ip -6 route show table $dns_table 2>/dev/null)\" ] && echo route6
        iptables -w 5 -t mangle -S $dns_chain >/dev/null 2>&1 && echo chain
        ip6tables -w 5 -t mangle -S $dns_chain >/dev/null 2>&1 && echo chain6
        iptables -w 5 -t mangle -S PREROUTING | grep -q $dns_chain && echo jump
        ip6tables -w 5 -t mangle -S PREROUTING | grep -q $dns_chain && echo jump6
        ip link show $dns_device >/dev/null 2>&1 && echo device
        true" | tr -d '\r' | xargs
}

check_dns() {
    local run blocked got started took supervisor daemon i family
    run="$(date +%s)$RANDOM"
    blocked="abd-dns-$run.$dns_zone"
    # A lookup is judged as the host of a connection that is not decrypted.
    # With URL filtering on, no host is blocked early while a web exception
    # in the lists could apply to any host, as the subscribed lists' can:
    # domain filtering is where the answer does not depend on them.
    adb shell "$ABD url-filtering off && echo '||$blocked^' >> $LIST && $ABD reload"
    for ((i = 0; i < 30; i++)); do
        adb shell "$ABD check https://probe.$blocked/" | grep -q '^blocked' && break
        sleep 1
    done
    await_dns_rule "$uid" 0
    dns_fixture=$PACKAGE.dns
    build_check_apk scripts/check/app dns --rename-manifest-package "$dns_fixture"
    adb install --user 0 "$work/dns.apk" >/dev/null

    expect "a selected app's lookup of a blocked host gets the unspecified address" \
        "resolved 0.0.0.0*" "$(look_up "a.$blocked")"
    expect "the selected app's lookup of another host resolves" \
        "resolved $dns_real" "$(look_up "allowed-$run.$dns_zone")"
    expect "the app's own blocked query over UDP is answered here, valid for no time" \
        "answer 0.0.0.0 ttl 0" "$(look_up "b.$blocked" "$PACKAGE" "--es dns udp")"
    expect "the app's own blocked query over TCP is answered here, valid for no time" \
        "answer 0.0.0.0 ttl 0" "$(look_up "c.$blocked" "$PACKAGE" "--es dns tcp")"
    expect "the app's own allowed query over TCP is answered by its server" \
        "answer $dns_real ttl *" "$(look_up "tcp-$run.$dns_zone" "$PACKAGE" "--es dns tcp")"
    # The selected app was just answered for a.$blocked: no other app gets
    # that answer from the cache the network's apps share.
    expect "an unselected app resolves the host the selected app was blocked from" \
        "resolved $dns_real" "$(look_up "a.$blocked" "$dns_fixture")"
    expect "root resolves a blocked host" "$dns_real" "$(root_look_up "d.$blocked")"
    adb shell "pm uninstall $dns_fixture" >/dev/null
    dns_fixture=

    adb shell "$ABD app pause $PACKAGE --minutes 1"
    await_dns_rule "$uid" 1
    expect "a paused app resolves a blocked host" "resolved $dns_real" "$(look_up "e.$blocked")"
    # A UDP socket connected while paused has its own address: once the app
    # is resumed, its queries keep their own route and are still answered.
    local files="/data/user/0/$PACKAGE/files"
    adb shell "rm -f $files/result $files/probe-ready $files/probe-go
        am start --user 0 -S -W -n $PACKAGE/.Main --es url 'kept-$run.$dns_zone' --es dns kept" >/dev/null
    for ((i = 0; i < 40; i++)); do adb shell "test -f $files/probe-ready" && break; sleep 0.5; done
    adb shell "$ABD app resume $PACKAGE"
    await_dns_rule "$uid" 0
    adb shell "touch $files/probe-go"
    for ((i = 0; i < 40; i++)); do
        got="$(adb shell "cat $files/result 2>/dev/null" | tr -d '\r' || true)"
        [[ -n $got ]] && break
        sleep 0.5
    done
    expect "a socket connected before the app was resumed is still answered" \
        "answer $dns_real ttl * / answer $dns_real ttl *" "${got:-no answer}"
    expect "the app's lookups are filtered again once resumed" "resolved 0.0.0.0*" "$(look_up "f.$blocked")"

    # The supervisor is held, so that nothing but the end of the daemon's
    # process can undo what it set up.
    supervisor=$(adb shell "cat $DATA/supervisor.pid" | tr -d '\r')
    daemon=$(adb shell "cat $DATA/abd.pid" | tr -d '\r')
    [[ $supervisor =~ ^[0-9]+$ && $daemon =~ ^[0-9]+$ ]] || die "the module is not running"
    adb shell "kill -STOP $supervisor"
    dns_held=$supervisor
    adb shell "kill -9 $daemon"
    for ((i = 0; i < 20; i++)); do
        [[ $(dns_leftovers) != *device* ]] && break
        sleep 0.5
    done
    got=$(dns_leftovers)
    [[ $got == rule* && $got == *chain* && $got != *route* && $got != *device* ]] &&
        got="rules left, device and routes gone"
    expect "a killed daemon's device and routes go with it" "rules left, device and routes gone" "$got"
    started=$(date +%s)
    got=$(look_up "g.$blocked")
    took=$(($(date +%s) - started))
    expect "the selected app resolves at once after the daemon is killed (${took}s with the app's start)" \
        "resolved $dns_real" "$got"
    adb shell "$ABD unhook"
    expect "abd unhook leaves nothing of lookup capture" "" "$(dns_leftovers)"
    adb shell "kill -CONT $supervisor"
    dns_held=

    # The supervisor backs off after a short run.
    for family in "${capture_families[@]}"; do await_gate "$family" 0 90; done
    await_quota 2
    await_dns_rule "$uid" 0 90
    expect "a new daemon filters the app's lookups again" "resolved 0.0.0.0*" "$(look_up "h.$blocked")"
    adb shell "$ABD url-filtering on"
    check_pinned_lookup
}

# With URL filtering on, a host the app pins is not decrypted, so its
# domain rule decides, for its lookups as for its connections. An
# exception that names the host keeps it from being blocked early. The app
# pins the host in its network security config, and refuses the certificate
# late enough after starting for the daemon to take it at its word. A try
# of the host at the app's next start does not change how it is judged.
check_pinned_lookup() {
    local host=example.net i
    adb shell "echo '||$host^' >> $LIST; echo '@@||$host/abd-check-any/\$image' >> $LIST; $ABD reload"
    for ((i = 0; i < 30; i++)); do
        adb shell "$ABD check https://$host/" | grep -q '^blocked' && break
        sleep 1
    done
    expect "a lookup of a host whose requests can be read is answered by its server" \
        "answer [1-9]* ttl *" "$(look_up "$host" "$PACKAGE" "--es dns udp")"
    expect "the app refuses the certificate for the host it pins" \
        "error *" "$(look_up "https://$host/" "$PACKAGE" "--ei delayMs 11000")"
    for ((i = 0; i < 10; i++)); do
        adb shell "$ABD pinned" | tr -d '\r' | grep -qx "$PACKAGE $host .*" && break
        sleep 1
    done
    expect "a lookup of a host the app pins is judged by its domain rule" \
        "answer 0.0.0.0 ttl 0" "$(look_up "$host" "$PACKAGE" "--es dns udp")"
    adb shell "$ABD pinned forget $PACKAGE $host; sed -i '/^||$host^\$/d; /abd-check-any/d' $LIST; $ABD reload"
}
