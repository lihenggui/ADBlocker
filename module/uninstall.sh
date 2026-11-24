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

# Root managers run this either at once, with the daemon running, or at the
# next boot, before anything of the module has started: it works both ways.
#
# Every step is taken whatever became of the one before. What matters most
# comes first: that no app is left without its network, then that no app
# trusts the authority any longer, then that its key is destroyed.
MODDIR=${0%/*}
ABD=$MODDIR/bin/abd
DATA=/data/adb/adblocker

stopped=0
"$ABD" module-stop || stopped=$?
if ! "$ABD" unhook; then
    printf '%s\n' "uninstall: abd unhook failed; capture rules may remain until the next boot" >&2
fi

# A daemon we could not stop could still mount a copy after the scan.
if [ "$stopped" -eq 0 ]; then
    "$ABD" trust remove
    removed=$?
else
    ABD_KEEP_TRUST_STORES=1 "$ABD" trust remove
    removed=$?
    # Kept, whatever it says.
    [ "$removed" -ne 0 ] || removed=3
fi
case "$removed" in
0) ;;
3) printf '%s\n' "uninstall: the store copies are kept while apps may still use them" >&2 ;;
*) printf '%s\n' "uninstall: abd trust remove failed ($removed); apps may trust the authority until they restart" >&2 ;;
esac

# Destroy the key even if trust cleanup could not be verified. The status
# is that of removing the data: what else did not finish is said above,
# for the root manager's log.
cleanup=0
rm -rf "$DATA/ca" || cleanup=1
if [ "$removed" -eq 0 ]; then
    rm -rf "$DATA" || cleanup=1
else
    for entry in "$DATA"/* "$DATA"/.[!.]*; do
        case "$entry" in
        "$DATA/store" | "$DATA/user-store") ;;
        *) rm -rf "$entry" || cleanup=1 ;;
        esac
    done
fi

# A module the app had installed that this one never came to replace would
# start again at the next boot, with nothing left of what it kept. Only run
# from a root manager's own directory is this the flashed module.
case "$MODDIR" in
/data/adb/modules/* | /data/adb/modules_update/*)
    if ! rm -rf /data/adb/adblocker_direct /data/adb/service.d/adblocker.sh; then
        printf '%s\n' "uninstall: the module the app had installed could not be removed" >&2
    fi
    ;;
esac
exit "$cleanup"
