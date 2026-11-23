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

# Sourced by check-device.sh with its test APK, kernel selector reader and cleanup trap.
create_check_profile() {
    local created
    created=$(adb shell pm create-user --profileOf 0 --managed ADBlockerCheck | tr -d '\r')
    profile_id=$(sed -n 's/^Success: created user id \([0-9][0-9]*\)$/\1/p' <<<"$created")
    [[ -n $profile_id && $profile_id != 0 ]] || die "could not create a disposable managed profile: $created"
    adb shell am start-user -w "$profile_id" >/dev/null
    adb shell cmd package install-existing --user "$profile_id" "$PACKAGE" >/dev/null
    profile_serial=$(adb shell dumpsys user | tr -d '\r' | sed -n "s/.*UserInfo{${profile_id}:[^}]*} serialNo=\([0-9][0-9]*\).*/\1/p")
    [[ $profile_serial =~ ^[1-9][0-9]*$ ]] || die "no serial for the test profile"
    work_key="$profile_serial:$PACKAGE"
}

remove_check_profile() {
    local users
    adb shell pm remove-user "$profile_id" >/dev/null
    for ((i = 0; i < 60; i++)); do
        users=$(adb shell pm list users) || die "could not query users after removing the test profile"
        if [[ $users != *"UserInfo{$profile_id:"* ]]; then
            profile_id=
            return
        fi
        sleep 1
    done
    die "the test profile was not removed"
}

profile_contains_app() {
    adb shell "$ABD app profiles --json" | python3 -c '
import json, sys
serial, package = int(sys.argv[1]), sys.argv[2]
profiles = json.load(sys.stdin)["profiles"]
sys.exit(not any(p["serial"] == serial and package in p["packages"] for p in profiles))
' "$profile_serial" "$PACKAGE"
}

work_level() {
    adb shell "$ABD app list --json" | python3 -c '
import json, sys
for app in json.load(sys.stdin):
    if app.get("profile_serial") == int(sys.argv[1]) and app["package"] == sys.argv[2]:
        levels = app["effective"]
        print(" / ".join("{} {}".format(levels[p]["level"], levels[p]["reason"] or "-") for p in ("http", "https")))
        break
' "$profile_serial" "$PACKAGE"
}

await_work_level() {
    local expected=$1 actual
    for ((i = 0; i < 30; i++)); do
        actual=$(work_level)
        [[ $actual == "$expected" ]] && return 0
        sleep 1
    done
    die "work filtering level was '$actual', expected '$expected'"
}

await_work_observers() {
    local expected=$1 count daemon_pid attempt
    for ((attempt = 0; attempt < 15; attempt++)); do
        daemon_pid=$(adb shell "cat $DATA/abd.pid" | tr -d '\r')
        [[ $daemon_pid =~ ^[1-9][0-9]*$ ]] || die "no running daemon to check"
        count=$(adb shell ps -A -o PPID,NAME | tr -d '\r' | awk -v daemon="$daemon_pid" '
            $1 == daemon && $2 ~ /(^|\/)logcat$/ { n++ }
            END { print n + 0 }
        ')
        [[ $count == "$expected" ]] && return 0
        sleep 1
    done
    die "daemon has $count work lifecycle observers, expected $expected"
}
check_idle_observer=$(adb shell "$ABD app list --json" | python3 -c '
import json, sys
print(int(all(app.get("profile_serial") is None for app in json.load(sys.stdin))))
')
if [[ $check_idle_observer == 1 ]]; then await_work_observers 0; fi

ADMIN=li.mercury.adblocker.check.admin
mkdir "$work/assets"
adb shell "$ABD ca export" > "$work/assets/test-ca.pem"
openssl x509 -in "$work/assets/test-ca.pem" -noout
build_check_apk scripts/check/admin admin -A "$work/assets"

adb shell "$ABD url-filtering off"
create_check_profile
profile_contains_app || die "the installed work app is missing from the profile inventory"
work_uid=$(adb shell "pm list packages --user $profile_id -U $PACKAGE" | tr -d '\r' | sed -n "s/^package:$PACKAGE uid://p")
[[ $work_uid =~ ^[0-9]+$ && $uid != "$work_uid" ]] || die "could not distinguish the two fixture UIDs"

adb shell "$ABD app add $work_key"
await_work_observers 1
await_selected_uid "$work_uid" 0
await_work_level "hosts url_filtering_off / hosts url_filtering_off"
expect "the newly selected work copy blocks its first host request" "error *" "$(fetch https://example.org/ "$profile_id")"
adb shell "$ABD app pause $PACKAGE --minutes 5"
await_selected_uid "$uid" 1
await_selected_uid "$work_uid" 0
expect "pausing the personal copy leaves the work rule active" "error *" "$(fetch https://example.org/ "$profile_id")"
expect "the unfiltered personal copy can reach the same host" "200 *" "$(fetch https://example.org/)"
adb shell "$ABD app resume $PACKAGE"
await_selected_uid "$uid" 0

expect "a work app with URL filtering off leaves the cleartext path rule to its server" "404 -" "$(fetch http://example.com/blocked-by-check "$profile_id")"
# The app list reflects the saved choice immediately. Use the personal
# fixture UID's independent re-entry as an acknowledgement from the daemon.
# The work UID remains selected while the global option changes.
adb shell "$ABD app pause $PACKAGE --minutes 5"
await_selected_uid "$uid" 1
adb shell "$ABD url-filtering on"
adb shell "$ABD app resume $PACKAGE"
await_selected_uid "$uid" 0
await_selected_uid "$work_uid" 0
await_work_level "full - / hosts profile_trust_unavailable"
expect "full work filtering preserves a web exception with no CA" "200 *" "$(fetch https://example.org/ "$profile_id")"
expect "work cleartext keeps full path rules without a CA" "403 -" "$(fetch http://example.com/blocked-by-check "$profile_id")"

# Only the disposable profile's administrator can opt it into this trust.
adb install --user "$profile_id" -r "$work/admin.apk" >/dev/null
adb shell dpm set-profile-owner --user "$profile_id" "$ADMIN/.CheckAdmin"
adb shell am start --user "$profile_id" -W -n "$ADMIN/.TrustCheckCa" >/dev/null
await_work_level "full - / full -"
await_selected_uid "$work_uid" 0
expect "the work copy uses its own authorized HTTPS trust" "403 *ADBlocker*" "$(fetch https://www.wikipedia.org/blocked-by-check "$profile_id")"

adb shell "$ABD app pause $work_key --minutes 5"
await_selected_uid "$work_uid" 1
await_selected_uid "$uid" 0
adb shell "$ABD app resume $work_key"
await_selected_uid "$work_uid" 0
adb shell am stop-user -w -f "$profile_id" >/dev/null
await_selected_uid "$work_uid" 1
adb shell am start-user -w "$profile_id" >/dev/null
await_selected_uid "$work_uid" 0
expect "work filtering resumes after the profile restarts" "403 *ADBlocker*" "$(fetch https://www.wikipedia.org/blocked-by-check "$profile_id")"

if [[ ${ABD_CHECK_WORK_TILE:-0} == 1 ]]; then
    source scripts/check/work-tile.sh
fi
source scripts/check/work-vpn.sh

# One system clock supplies the day boundaries used by the stats command.
epoch=$(adb shell date +%s | tr -d '\r')
[[ $epoch =~ ^[0-9]+$ ]] || die "could not read the device clock"
edges=()
today=$((epoch / 86400 * 86400))
for ((day = -6; day <= 1; day++)); do edges+=("$((today + day * 86400))"); done
for ((attempt = 0; attempt < 40; attempt++)); do
    if adb shell "$ABD" stats --json "${edges[@]}" | python3 -c '
import json, sys
apps = json.load(sys.stdin)["apps"]
sys.exit(not all(any(sum(day) for day in apps.get(key, [])) for key in sys.argv[1:]))
' "$PACKAGE" "$work_key"; then break; fi
    sleep 1
done
((attempt < 40)) || die "personal and work block counts did not appear under separate app keys"
echo "ok    personal and work block counts have separate app identities"

adb shell "$ABD app remove $PACKAGE"
adb shell pm uninstall --user 0 "$PACKAGE" >/dev/null
profile_contains_app || die "a work-only app disappeared from the inventory"
await_selected_uid "$work_uid" 0
expect "an app installed only at work remains selectable and filtered" "403 *ADBlocker*" "$(fetch https://www.wikipedia.org/blocked-by-check "$profile_id")"
adb shell am start --user "$profile_id" -W -n "$ADMIN/.TrustCheckCa" --ez remove true >/dev/null
await_work_level "full - / hosts profile_trust_unavailable"
await_selected_uid "$work_uid" 0
expect "removing the work CA preserves its allowed HTTPS connection" "200 *" "$(fetch https://example.org/ "$profile_id")"
old_serial=$profile_serial
adb shell "$ABD app pause $work_key --minutes 5"
await_selected_uid "$work_uid" 1
remove_check_profile
# A deleted profile cannot be resumed. Its selection and pause should
# disappear together even if an unrelated package is installed later.
for ((attempt = 0; attempt < 30; attempt++)); do
    retained=$(adb shell "for path in $DATA/apps $DATA/pauses; do
        if [ -f \$path ]; then grep -F '$work_key' \$path; fi
    done; echo end")
    [[ $retained == end ]] && break
    sleep 1
done
[[ $retained == end ]] || die "deleted profile still has a stored selection or pause"
await_work_level ""
echo "ok    deleting the work profile removes its selection and pause"
# Retain a copy of the APK for install-existing in this fresh profile.
adb install --user 0 -r "$work/check.apk" >/dev/null
create_check_profile
[[ $profile_serial != "$old_serial" ]] || die "the new profile reused the deleted profile's serial"
[[ -z $(work_level) ]] || die "the new profile inherited a deleted profile's app selection"
expect "a recreated profile does not inherit filtering" "200 *" "$(fetch https://example.org/ "$profile_id")"
remove_check_profile
if [[ $check_idle_observer == 1 ]]; then
    await_work_observers 0
    echo "ok    the daemon releases the work lifecycle observer after its final work selection"
fi
