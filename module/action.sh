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

# Action button in the root manager: show the daemon's status.
case "$0" in
*/*) MODDIR=${0%/*} ;;
*) MODDIR=. ;;
esac
DATA=/data/adb/adblocker

pid="$(cat "$DATA/abd.pid" 2>/dev/null)"
case "$pid" in
'' | *[!0-9]*) pid=0 ;;
esac
if [ "$pid" -gt 1 ] 2>/dev/null && kill -0 "$pid" 2>/dev/null &&
    [ "$(cat /proc/"$pid"/comm 2>/dev/null)" = abd ]; then
    echo "Daemon: running (pid $pid)"
else
    echo "Daemon: NOT running"
fi
echo
echo "Configured apps:"
"$MODDIR/bin/abd" app list 2>&1
echo
# abd stats begins with the date the counts start from.
echo "Counts:"
"$MODDIR/bin/abd" stats 2>&1
echo
echo "Last log lines:"
tail -n 15 "$DATA/abd.log" 2>&1
