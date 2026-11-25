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

# Keeps the daemon running: restarts it if it dies, and gives up if it keeps
# dying right after starting (a crash loop would only drain the battery).
#
# Disarm and remove the packet filter rules after the daemon exits. Stale
# rules can divert connections when no daemon is listening.
case "$0" in
*/*) MODDIR=${0%/*} ;;
*) MODDIR=. ;;
esac
DATA=/data/adb/adblocker
LOG=$DATA/abd.log
ABD=$MODDIR/bin/abd

# A run shorter than this many seconds counts as a failed start.
MIN_UPTIME=30
MAX_FAILED_STARTS=5
# Seconds before a daemon that died is started again. The first time, at
# once: while there is no daemon nothing is filtered. Then FIRST_DELAY,
# twice as long after each further restart, up to MAX_DELAY; and at once
# again once a run has lasted HEALTHY_UPTIME.
FIRST_DELAY=5
MAX_DELAY=600
HEALTHY_UPTIME=600

log() {
    message="--- $(date '+%F %T') $*"
    printf '%s\n' "$message" >>"$LOG" || printf '%s\n' "$message" >&2 || :
}

# Seconds since boot. Unlike the wall clock, this does not jump when the
# time is set during boot.
uptime_seconds() {
    cut -d. -f1 /proc/uptime
}

unhook_once() {
    attempted=0
    if {
        attempted=1
        "$ABD" unhook
    } 2>>"$LOG"; then
        return 0
    fi
    # A redirection failure must not keep the cleanup command from running.
    [ "$attempted" -eq 0 ] && "$ABD" unhook
}

# Retry cleanup when the system holds the packet filter tables' locks.
unhook() {
    tries=0
    until unhook_once; do
        tries=$((tries + 1))
        if [ "$tries" -ge 10 ]; then
            log "packet filter cleanup failed after $tries attempts"
            return 1
        fi
        sleep 1
    done
}

# If publication failed, a stop request cannot find this supervisor.
published=
owns_pid_file() {
    [ -f "$DATA/supervisor.pid" ] &&
        IFS= read -r published <"$DATA/supervisor.pid" && [ "$published" = "$$" ]
}
for attempt in 1 2 3; do
    owns_pid_file && break
    published=
    [ "$attempt" -eq 3 ] || sleep 1
done
if [ "$published" != "$$" ]; then
    log "supervisor PID was not published"
    unhook
    exit 1
fi

pid=
sleeping=
clear_pids() {
    owns_pid_file || return 0
    rm -f "$DATA/supervisor.pid" "$DATA/abd.pid" || log "could not clear the process PID files"
}
trap clear_pids EXIT

stop_child() {
    kill "$pid" 2>/dev/null || return 0
    # Leave time for unhook before module-stop can kill this supervisor.
    for attempt in 1 2 3 4; do
        kill -0 "$pid" 2>/dev/null || return 0
        sleep 1
    done
    if kill -0 "$pid" 2>/dev/null; then
        log "daemon did not stop in time; killing it"
        kill -9 "$pid" 2>/dev/null || :
    fi
}

# Asked to stop: take the daemon down too, and wait while it removes its
# rules and writes its counts.
stop() {
    trap '' TERM INT
    [ -z "$pid" ] || stop_child
    [ -n "$sleeping" ] && kill "$sleeping" 2>/dev/null
    unhook
    exit "$?"
}
trap stop TERM INT

start_daemon() {
    ABD_EXPECT_PID=1 "$ABD" run 3>&- >>"$LOG" 2>&1 &
    pid=$!
    printf '%s\n' "$pid" >&3
}

failed=0
delay=0
pid_file=$DATA/abd.pid
while :; do
    started=$(uptime_seconds)
    if { [ -e "$pid_file" ] && [ ! -f "$pid_file" ]; } || ! start_daemon 3>"$pid_file"; then
        log "could not publish the daemon PID"
        # The child cannot start filtering without publication. Disarm
        # any old rules before waiting for its bounded startup check.
        unhook || :
        [ -z "$pid" ] || wait "$pid" 2>/dev/null || :
        exit 1
    fi
    code=0
    wait "$pid" || code=$?
    # Gone: its pid may be handed to another process.
    pid=
    ran=$(($(uptime_seconds) - started))
    # A daemon that was killed left its rules behind.
    unhook || exit 1
    owns_pid_file || exit 0
    if ! rm -f "$pid_file"; then
        log "could not clear the daemon PID after exit"
        exit 1
    fi

    # The daemon exits with status 0 only when it is asked to stop.
    if [ "$code" -eq 0 ]; then
        log "daemon exited normally after ${ran}s; not restarting"
        break
    fi

    if [ "$ran" -lt "$MIN_UPTIME" ]; then
        failed=$((failed + 1))
    else
        failed=0
    fi
    if [ "$failed" -ge "$MAX_FAILED_STARTS" ]; then
        log "daemon exited with status $code after ${ran}s; giving up after $failed failed starts"
        break
    fi
    [ "$ran" -ge "$HEALTHY_UPTIME" ] && delay=0
    log "daemon exited with status $code after ${ran}s; restarting in ${delay}s"
    if [ "$delay" -gt 0 ]; then
        # In the background, so that a request to stop is acted on at once.
        sleep "$delay" &
        sleeping=$!
        wait "$sleeping"
        sleeping=
        delay=$((delay * 2))
        [ "$delay" -gt "$MAX_DELAY" ] && delay=$MAX_DELAY
    else
        delay=$FIRST_DELAY
    fi
done
