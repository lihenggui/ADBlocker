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
        boot-completes completed-clear stop-fails data-fails supervisor-write supervisor-gate unhook-no-log \
        daemon-write basename-stat successor-pids stop-child action-pid; do
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
for script in service supervise stop action; do
    {
        # An orphaned test waiter can still see boot completion after cleanup.
        printf '%s\n' 'getprop() { if [ ! -d "$FIXTURE" ] || [ -f "$FIXTURE/booted" ]; then echo 1; else echo 0; fi; }'
        if [ "$script" = action ]; then
            printf '%s\n' 'kill() { printf "%s\n" "$*" >>"$FIXTURE/signals"; }'
        fi
        # The module the app may have installed is looked for under the fixture too.
        sed -e 's|^DATA=/data/adb/adblocker$|DATA=$FIXTURE/data|' -e 's|/data/adb/|$FIXTURE/adb/|g' "module/$script.sh"
    } >"$module/$script.sh"
done
: >"$fixture/calls"

cat >"$module/bin/abd" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$FIXTURE/calls"
case "$SCENARIO:$*" in
    successor-pids:run)
        printf '87654\n' >"$FIXTURE/data/supervisor.pid"
        printf '87655\n' >"$FIXTURE/data/abd.pid" ;;
    stop-child:run)
        trap '' TERM
        touch "$FIXTURE/child-ready"
        while :; do sleep 1; done ;;
esac
test "$SCENARIO:$*" != stop-fails:module-stop
EOF
cat >"$fixture/tools/cat" <<'EOF'
#!/bin/sh
if [ "$SCENARIO:$*" = 'action-pid:/proc/42/comm' ]; then echo abd; exit 0; fi
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
cat >"$fixture/tools/stat" <<'EOF'
#!/bin/sh
if [ "$SCENARIO" = basename-stat ]; then
    touch "$FIXTURE/stat-called"
    printf '%s\n' 'mock stat error' >&2
    exit 1
fi
exec /usr/bin/stat "$@"
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
        if (cd "$module" && sh -c 'printf "%s\n" "$$" >"$FIXTURE/data/supervisor.pid"
                  exec sh supervise.sh') >"$fixture/output" 2>&1; then
            fail "supervisor ignored a failed daemon PID write"
        fi
        grep -qx unhook "$fixture/calls" || fail "daemon PID failure did not unhook"
        no_start
        ;;
    basename-stat)
        touch "$fixture/booted" "$data/abd.log"
        (cd "$module" && sh -e service.sh) >"$fixture/output" 2>&1 || fail "service failed"
        test -e "$fixture/stat-called" || fail "rotation did not check the log"
        test ! -s "$fixture/output" || fail "stat failure leaked output or caused a shell error"
        for attempt in 1 2 3 4 5; do
            if grep -qx run "$fixture/calls" && [ ! -e "$data/supervisor.pid" ]; then exit 0; fi
            sleep 1
        done
        fail "the daemon did not start from a basename"
        ;;
    successor-pids)
        sh -c 'printf "%s\n" "$$" >"$FIXTURE/data/supervisor.pid"
               exec sh "$FIXTURE/module/supervise.sh"' >"$fixture/output" 2>&1 ||
            fail "the old supervisor did not exit"
        test "$(cat "$data/supervisor.pid")" = 87654 &&
            test "$(cat "$data/abd.pid")" = 87655 || fail "the successor lost its PID files"
        grep -qx unhook "$fixture/calls" || fail "PID ownership prevented unhook"
        ;;
    stop-child)
        sh -c 'printf "%s\n" "$$" >"$FIXTURE/data/supervisor.pid"
               exec sh "$FIXTURE/module/supervise.sh"' >"$fixture/output" 2>&1 &
        supervisor=$!
        trap '
            if [ -n "${child:-}" ]; then kill -KILL "$child" 2>/dev/null || :; fi
            if [ -n "$supervisor" ]; then kill -KILL "$supervisor" 2>/dev/null || :; fi
            rm -rf "$fixture"
        ' EXIT
        for attempt in 1 2 3 4 5; do
            if [ -e "$fixture/child-ready" ] && [ -s "$data/abd.pid" ]; then break; fi
            sleep 1
        done
        test -e "$fixture/child-ready" || fail "the child did not start"
        child=$(cat "$data/abd.pid")
        kill -TERM "$supervisor"
        # On a stuck daemon the external caller kills the supervisor at 5s.
        for attempt in 1 2 3 4 5; do
            sleep 1
            if grep -qx unhook "$fixture/calls"; then
                wait "$supervisor" || fail "the supervisor failed to stop"
                supervisor=
                test ! -e "$data/abd.pid" && test ! -e "$data/supervisor.pid" ||
                    fail "the supervisor did not clear its PID files"
                child=
                exit 0
            fi
        done
        fail "unhook was blocked by the child"
        ;;
    action-pid)
        touch "$data/abd.log"
        for invalid in -1 0 1 '42/invalid'; do
            printf '%s\n' "$invalid" >"$data/abd.pid"
            (cd "$module" && sh action.sh) >"$fixture/output" 2>&1 ||
                fail "the action failed"
            grep -qx 'Daemon: NOT running' "$fixture/output" || fail "an invalid PID was accepted"
        done
        test ! -e "$fixture/signals" || fail "an invalid PID was sent to kill"
        printf '42\n' >"$data/abd.pid"
        (cd "$module" && sh action.sh) >"$fixture/output" 2>&1 || fail "the action failed"
        grep -qx 'Daemon: running (pid 42)' "$fixture/output" || fail "a daemon PID was not accepted"
        grep -qx -e '-0 42' "$fixture/signals" || fail "the daemon PID was not checked"
        grep -qx 'app list' "$fixture/calls" && grep -qx 'stats' "$fixture/calls" ||
            fail "the action did not find the daemon from its basename"
        ;;
esac
