#!/system/bin/sh
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

# The test origin lives on this emulator, beyond a veth pair. The client sees
# a unicast peer on the underlying Android network even when its VPN is active.
set -eu
action=$1
stage=$2
local_if=abdh3check
peer_if=abdh3peer
fixture_table=31741
case "$action" in
    start)
        test ! -e "/sys/class/net/$local_if"
        # The child publishes its own PID after entering the network namespace.
        # shellcheck disable=SC2016
        setsid unshare -n sh -c 'echo $$ > "$1/ns.pid"; exec sleep 600' sh "$stage" </dev/null >"$stage/ns.log" 2>&1 &
        for _ in $(seq 1 50); do test ! -s "$stage/ns.pid" || break; sleep .1; done
        peer=$(cat "$stage/ns.pid")
        ns="/proc/$peer/ns/net"
        test -e "$ns" && test "$(readlink "$ns")" != "$(readlink /proc/self/ns/net)" || exit 1
        ip link add "$local_if" type veth peer name "$peer_if"
        touch "$stage/created"
        ip link set "$peer_if" netns "$peer"
        nsenter --net="$ns" -- ip link set "$peer_if" up
        nsenter --net="$ns" -- ip addr add 198.18.0.2/30 dev "$peer_if"
        nsenter --net="$ns" -- ip -6 addr add 2001:db8:ad::2/64 dev "$peer_if" nodad
        ip link set "$local_if" up
        ip addr add 198.18.0.1/30 dev "$local_if"
        ip -6 addr add 2001:db8:ad::1/64 dev "$local_if" nodad
        tables=$(ip route show table all | awk '$1 == "default" && $2 == "via" {
            for(i=1;i<NF;i++) if ($i == "table") print $(i+1)
        }' | sort -u)
        if [ -z "$tables" ]; then
            # The kernel fixture may boot without a modem or Wi-Fi route.
            # Give only the fixture apps routes to the two local peers.
            case "$3" in root) ;; *) echo "The VPN check needs an Android network" >&2; exit 1 ;; esac
            package=li.mercury.adblocker.check.http3
            uid_list=
            for app in "$package" "$package.control"; do
                uid=$(pm list packages -U --user 0 "$app" |
                    awk -v key="package:$app" '$1 == key { sub(/^uid:/, "", $2); print $2 }')
                case "$uid" in 1[0-9][0-9][0-9][0-9]) ;; *) exit 1 ;; esac
                uid_list="$uid_list $uid"
            done
            for family in 4 6; do
                case "$family" in
                    4) dest=198.18.0.2/32; source=198.18.0.1 ;;
                    6) dest=2001:db8:ad::2/128; source=2001:db8:ad::1 ;;
                esac
                ip -"$family" route add "$dest" dev "$local_if" src "$source" table "$fixture_table"
                for uid in $uid_list; do
                    ip -"$family" rule add pref 31999 to "$dest" uidrange "$uid-$uid" lookup "$fixture_table"
                    echo "$uid" >> "$stage/route$family"
                done
            done
        fi
        for table in $tables; do
            ip route add 198.18.0.2/32 dev "$local_if" src 198.18.0.1 table "$table"
            ip -6 route add 2001:db8:ad::2/128 dev "$local_if" src 2001:db8:ad::1 table "$table"
        done
        ;;
    origin|tcp-origin)
        family=$3
        case "$family" in
            4) addr=198.18.0.2:443 ;;
            6) addr='[2001:db8:ad::2]:443' ;;
            *) exit 2 ;;
        esac
        ns="/proc/$(cat "$stage/ns.pid")/ns/net"
        if test "$action" = tcp-origin; then
            exec nsenter --net="$ns" -- "$stage/check" tcp-origin "${addr%:443}:80"
        fi
        exec nsenter --net="$ns" -- "$stage/check" origin "$addr" "$stage/cert.der" "$stage/key.der"
        ;;
    stop)
        for family in 4 6; do
            if [ -f "$stage/route$family" ]; then
                case "$family" in 4) dest=198.18.0.2/32 ;; 6) dest=2001:db8:ad::2/128 ;; esac
                while IFS= read -r uid; do
                    ip -"$family" rule del pref 31999 to "$dest" uidrange "$uid-$uid" lookup "$fixture_table"
                done < "$stage/route$family"
            fi
        done
        if test -e "$stage/created"; then ip link del "$local_if"; fi
        if test -s "$stage/ns.pid"; then
            peer=$(cat "$stage/ns.pid")
            if test "$(cat /proc/"$peer"/comm 2>/dev/null)" = sleep; then kill "$peer"; fi
        fi
        ;;
    *) exit 2 ;;
esac
