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

# Opt-in device acceptance on an unlocked emulator with English SystemUI.
# Find the stock Work apps tile in the actual Quick Settings hierarchy.
work_tile() {
    local page action x1 y1 x2 y2 attempt
    adb shell input keyevent 3
    adb shell cmd statusbar expand-settings
    for ((attempt = 0; attempt < 8; attempt++)); do
        sleep 0.5
        page=$(adb shell 'path=$(mktemp /data/local/tmp/abd-work-tile.XXXXXX)
            uiautomator dump "$path" >/dev/null && cat "$path"
            rm -f "$path"' | python3 -c '
import re, sys, xml.etree.ElementTree as xml
try:
    nodes = list(xml.fromstring(sys.stdin.read()).iter("node"))
except xml.ParseError:
    sys.exit(0)

def bounds(node):
    return tuple(map(int, re.findall(r"[0-9]+", node.get("bounds", ""))))

for node in nodes:
    if (node.get("package") == "com.android.systemui"
            and node.get("clickable") == "true"
            and node.get("content-desc") in ("Work apps", "Work profile")):
        x1, y1, x2, y2 = bounds(node)
        print("tap", (x1 + x2) // 2, (y1 + y2) // 2)
        sys.exit(0)
for node in nodes:
    if node.get("resource-id") == "com.android.systemui:id/qs_pager":
        x1, y1, x2, y2 = bounds(node)
        margin = (x2 - x1) // 8
        y = (y1 + y2) // 2
        print("swipe", x2 - margin, y, x1 + margin, y)
        break
')
        read -r action x1 y1 x2 y2 <<<"$page"
        case $action in
            tap)
                adb shell input tap "$x1" "$y1"
                echo "ok    tapped the SystemUI Work apps tile at $x1 $y1"
                return
                ;;
            swipe) adb shell input swipe "$x1" "$y1" "$x2" "$y2" 350 ;;
            *) adb shell cmd statusbar expand-settings ;;
        esac
    done
    die "the SystemUI Work apps tile was not visible"
}

await_quiet_flag() {
    local want=$1 flags attempt
    for ((attempt = 0; attempt < 30; attempt++)); do
        flags=$(adb shell dumpsys user | awk -v id="$profile_id" '
            /UserInfo\{/ { found = index($0, "UserInfo{" id ":") != 0 }
            found && /^[[:space:]]+Flags:/ { print; found = 0 }
        ')
        if [[ -n $flags ]]; then
            if [[ $want == yes && $flags == *QUIET_MODE* ]] ||
                [[ $want == no && $flags != *QUIET_MODE* ]]; then
                echo "ok    work profile quiet=$want ($flags)"
                return
            fi
        fi
        sleep 1
    done
    die "work profile quiet flag did not match $want"
}

await_quiet_flag no
work_tile
await_quiet_flag yes
await_selected_uid "$work_uid" 1
await_selected_uid "$uid" 0
adb shell cmd statusbar collapse
expect "the personal copy stays online while Work apps is off" "200 *" "$(fetch https://example.com/)"

work_tile
await_quiet_flag no
await_selected_uid "$work_uid" 0
adb shell cmd statusbar collapse
expect "the saved work filter returns once Work apps is on" "403 *ADBlocker*" "$(fetch https://www.wikipedia.org/blocked-by-check "$profile_id")"
