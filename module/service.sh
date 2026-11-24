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

# Starts the daemon under its supervisor. Runs at late_start, and again after a hot install, in which
# case a previous instance is stopped first.
MODDIR=${0%/*}
DATA=/data/adb/adblocker
LOG=$DATA/abd.log

# Root managers and adb shells can supply different inherited umasks. The
# data holds the key of an authority the filtered apps trust.
umask 077
. "$MODDIR/stop.sh"
mkdir -p "$DATA" && chmod 700 "$DATA" || exit 1

log() {
    message="--- $(date '+%F %T') $*"
    printf '%s\n' "$message" >>"$LOG" || printf '%s\n' "$message" >&2 || :
}

# Flashed in a root manager over the module the app had installed: this one
# takes over. stop.sh has stopped that one's daemon, by the pid files every
# installation shares. Only run from a manager's own directory is this a
# flashed module: the app's, started by another name for where it is, would
# otherwise remove itself.
case "$MODDIR" in
/data/adb/modules/* | /data/adb/modules_update/*)
    if [ -e /data/adb/adblocker_direct ]; then
        rm -rf /data/adb/adblocker_direct /data/adb/service.d/adblocker.sh ||
            log "could not remove the module the app had installed"
    fi
    ;;
esac

# Boot guard. A marker is left while the system boots and removed once it has
# finished; it holds how many boots in a row started the module and never
# finished. After two the module disables itself rather than risk a third.
# One alone is not held against it: a phone switched off while it boots, or
# out of power, does that with no module to blame. Not applied when started
# after boot (a hot install).
BOOT_MARKER=$DATA/boot_pending
BOOT_GUARD=$DATA/boot-guard
if [ "$(getprop sys.boot_completed)" != 1 ]; then
    unfinished=0
    if [ -e "$BOOT_MARKER" ] || [ -L "$BOOT_MARKER" ]; then
        if [ ! -f "$BOOT_MARKER" ] || ! unfinished=$(cat "$BOOT_MARKER"); then
            log "could not read the pending boot marker"
            exit 1
        fi
        # Only 0, 1 and at least 2 matter. Avoid arithmetic on unchecked
        # values, and count a nonnumeric marker as one unfinished boot.
        case "$unfinished" in
        '' | *[!0-9]*) unfinished=1 ;;
        *[1-9]*[0-9] | *[2-9]) unfinished=2 ;;
        *1) unfinished=1 ;;
        *) unfinished=0 ;;
        esac
    fi
    if [ "$unfinished" -ge 2 ]; then
        failed=0
        # For `abd status` to say why nothing runs, and since when.
        if { [ -e "$BOOT_GUARD" ] && [ ! -f "$BOOT_GUARD" ]; } ||
            ! date +%s >"$BOOT_GUARD"; then
            log "could not write the boot guard timestamp"
            failed=1
        fi
        if touch "$MODDIR/disable" && [ -f "$MODDIR/disable" ]; then
            log "two boots in a row did not complete; module disabled"
            if ! rm -f "$BOOT_MARKER"; then
                log "could not clear the pending boot marker"
                failed=1
            fi
        else
            # Preserve the retry count so the module stays off next boot.
            log "could not write the module disable marker"
            failed=1
        fi
        exit "$failed"
    fi
    if [ "$unfinished" -ge 1 ]; then
        log "previous boot did not complete; one more and the module is disabled"
    fi
    if ! printf '%s\n' "$((unfinished + 1))" >"$BOOT_MARKER"; then
        log "could not write the pending boot marker"
        exit 1
    fi
    # Waited for here rather than in a boot-completed.sh, which Magisk does
    # not run. A boot that hangs takes this process down with it, and the
    # marker stays.
    (
        while [ "$(getprop sys.boot_completed)" != 1 ]; do
            sleep 2
        done
        if ! rm -f "$BOOT_MARKER"; then
            log "could not clear the pending boot marker after boot"
            exit 1
        fi
    ) &
fi

# Start a new log once the old one passes 1 MiB. Checked at each start only.
if [ -f "$LOG" ] && [ "$(stat -c %s "$LOG")" -gt 1048576 ]; then
    mv -f "$LOG" "$LOG.old"
fi

# Started again, by whoever turned the module back on.
if ! rm -f "$BOOT_GUARD"; then
    log "could not clear the boot guard timestamp"
    exit 1
fi

start_supervisor() {
    sh "$MODDIR/supervise.sh" 3>&- &
    printf '%s\n' "$!" >&3
}

# Open the file before spawning. The supervisor waits for its own PID to
# appear before it can start a daemon.
log "starting"
pid_file=$DATA/supervisor.pid
if { [ -e "$pid_file" ] && [ ! -f "$pid_file" ]; } || ! start_supervisor 3>"$pid_file"; then
    log "could not publish the supervisor PID"
    "$MODDIR/bin/abd" unhook || :
    exit 1
fi
