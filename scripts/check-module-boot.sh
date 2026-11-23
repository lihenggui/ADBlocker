#!/bin/sh
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

set -eu
if [ "$#" -eq 0 ]; then
    result=0
    for scenario in pending-read pending-write pending-clear disabled disable-marker guard-write guard-clear \
        boot-completes completed-clear stop-fails data-fails supervisor-write supervisor-gate unhook-no-log daemon-write; do
        sh "$0" "$scenario" || result=1
    done
    exit "$result"
fi

cd "$(dirname "$0")/.."
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
export FIXTURE="$fixture" SCENARIO="$1"
data=$fixture/data
module=$fixture/module
mkdir -p "$data" "$module/bin" "$fixture/tools"
for script in service supervise stop; do
    {
        # An orphaned test waiter can still see boot completion after cleanup.
        printf '%s\n' 'getprop() { if [ ! -d "$FIXTURE" ] || [ -f "$FIXTURE/booted" ]; then echo 1; else echo 0; fi; }'
        sed 's|^DATA=/data/adb/adblocker$|DATA=$FIXTURE/data|' "module/$script.sh"
    } >"$module/$script.sh"
done
: >"$fixture/calls"

cat >"$module/bin/abd" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$FIXTURE/calls"
test "$SCENARIO:$*" != stop-fails:module-stop
EOF
cat >"$fixture/tools/cat" <<'EOF'
#!/bin/sh
command -p cat "$@" || exit "$?"
if [ "$SCENARIO" = pending-write ] && [ "$*" = "$FIXTURE/data/boot_pending" ]; then
    rm "$1" && mkdir "$1"
fi
EOF
cat >"$fixture/tools/rm" <<'EOF'
#!/bin/sh
case "$SCENARIO:$*" in
    "pending-clear:-f $FIXTURE/data/boot_pending" | "completed-clear:-f $FIXTURE/data/boot_pending") exit 1 ;;
esac
exec /bin/rm "$@"
EOF
chmod +x "$module/bin/abd" "$fixture/tools/"*
PATH=$fixture/tools:$PATH
export PATH

fail() {
    echo "$SCENARIO: $*" >&2
    if [ -f "$fixture/output" ]; then cat "$fixture/output" >&2; fi
    if [ -f "$data/abd.log" ]; then cat "$data/abd.log" >&2; fi
    exit 1
}

no_start() {
    if grep -qx run "$fixture/calls"; then fail "the daemon started"; fi
}

service_fails() {
    if sh "$@" "$module/service.sh" >"$fixture/output" 2>&1; then
        fail "service succeeded"
    fi
    no_start
}

case "$SCENARIO" in
    pending-read)
        mkdir "$data/boot_pending"
        service_fails
        ;;
    pending-write)
        printf '1\n' >"$data/boot_pending"
        service_fails
        test -d "$data/boot_pending" || fail "write failure was not injected"
        ;;
    pending-clear)
        printf '2\n' >"$data/boot_pending"
        service_fails
        test -f "$module/disable" && test -f "$data/boot_pending" || fail "boot state was lost"
        ;;
    disabled)
        printf '12345678901234567890123456789012345678901234567890\n' >"$data/boot_pending"
        sh -e "$module/service.sh" >"$fixture/output" 2>&1 || fail "disable failed"
        test -f "$module/disable" && test -s "$data/boot-guard" && test ! -e "$data/boot_pending" ||
            fail "the disable metadata was not published"
        no_start
        ;;
    disable-marker)
        mkdir "$module/disable"
        printf '2\n' >"$data/boot_pending"
        service_fails
        test "$(cat "$data/boot_pending")" = 2 && test -s "$data/boot-guard" ||
            fail "disable failure lost the retry marker or the guard"
        ;;
    guard-write)
        mkdir "$data/boot-guard"
        printf '2\n' >"$data/boot_pending"
        service_fails
        test -f "$module/disable" && test ! -e "$data/boot_pending" ||
            fail "guard write prevented disable or marker cleanup"
        ;;
    guard-clear)
        touch "$fixture/booted"
        mkdir "$data/boot-guard"
        service_fails
        ;;
    boot-completes)
        printf 'invalid\n' >"$data/boot_pending"
        sh -e "$module/service.sh" >"$fixture/output" 2>&1 || fail "service failed"
        test "$(cat "$data/boot_pending")" = 2 || fail "invalid marker did not count as one boot"
        touch "$fixture/booted"
        for attempt in 1 2 3 4 5; do
            if [ ! -e "$data/boot_pending" ] && grep -qx run "$fixture/calls" &&
                [ ! -e "$data/supervisor.pid" ] && [ ! -e "$data/abd.pid" ]; then
                exit 0
            fi
            sleep 1
        done
        fail "boot or daemon exit metadata was not cleared"
        ;;
    completed-clear)
        sh "$module/service.sh" >"$fixture/output" 2>&1 || fail "service failed"
        touch "$fixture/booted"
        for attempt in 1 2 3 4 5; do
            if grep -q 'could not clear the pending boot marker after boot' "$data/abd.log"; then
                test -f "$data/boot_pending" || fail "the test did not retain the marker"
                exit 0
            fi
            sleep 1
        done
        fail "the background boot waiter did not report the failed cleanup"
        ;;
    stop-fails)
        service_fails -e
        printf '%s\n' module-stop unhook | cmp - "$fixture/calls" ||
            fail "unhook did not run after module-stop failed"
        ;;
    data-fails)
        rmdir "$data"
        touch "$data"
        service_fails
        grep -qx unhook "$fixture/calls" || fail "unhook depended on writable data"
        ;;
    supervisor-write)
        touch "$fixture/booted"
        mkdir "$data/supervisor.pid"
        service_fails
        test "$(grep -cx unhook "$fixture/calls")" = 2 || fail "PID failure did not unhook"
        ;;
    supervisor-gate | unhook-no-log)
        if [ "$SCENARIO" = unhook-no-log ]; then mkdir "$data/abd.log"; fi
        if sh "$module/supervise.sh" >"$fixture/output" 2>&1; then
            fail "supervisor ran without its published PID"
        fi
        grep -qx unhook "$fixture/calls" || fail "supervisor gate failure did not unhook"
        no_start
        ;;
    daemon-write)
        mkdir "$data/abd.pid"
        if sh -c 'printf "%s\n" "$$" >"$FIXTURE/data/supervisor.pid"
                  exec sh "$FIXTURE/module/supervise.sh"' >"$fixture/output" 2>&1; then
            fail "supervisor ignored a failed daemon PID write"
        fi
        grep -qx unhook "$fixture/calls" || fail "daemon PID failure did not unhook"
        no_start
        ;;
esac
