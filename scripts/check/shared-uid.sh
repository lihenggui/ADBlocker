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

# Sourced by check-device.sh while the independent control app is selected.
(
    first="$PACKAGE.uid.first"
    second="$PACKAGE.uid.second"
    third="$PACKAGE.uid.third"
    # A previous run may still have unflushed statistics for its fixture.
    shared_id="$PACKAGE.uid.t${work##*.}"
    group="@$shared_id"
    blocked_host=http://example.org/
    blocked_path=http://example.com/blocked-by-check
    stage="$work/shared-uid"
    mkdir "$stage"
    group_user=
    group_work_key=
    installed=()
    cleanup_group() {
        if [[ -n $group_work_key ]]; then
            adb shell "$ABD app remove $group_work_key" >/dev/null 2>&1 || true
        fi
        if [[ -n $group_user ]]; then
            adb shell pm remove-user "$group_user" >/dev/null 2>&1 || true
        fi
        adb shell "$ABD app remove $group" >/dev/null 2>&1 || true
        for pkg in "${installed[@]}"; do
            adb uninstall "$pkg" >/dev/null 2>&1 || true
        done
    }
    trap cleanup_group EXIT

    install_member() {
        local package=$1 member=${1##*.}
        if [[ ! -f $work/shared-$member.apk ]]; then
            local source="$stage/$member"
            mkdir "$source"
            cp -R scripts/check/app/. "$source/"
            cat > "$source/AndroidManifest.xml" <<EOF
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="$package" android:sharedUserId="$shared_id">
    <uses-sdk android:minSdkVersion="31" android:targetSdkVersion="35" />
    <uses-permission android:name="android.permission.INTERNET" />
    <application android:label="Shared UID check" android:networkSecurityConfig="@xml/network_security_config">
        <activity android:name="$PACKAGE.Main" android:exported="true" />
    </application>
</manifest>
EOF
            build_check_apk "$source" "shared-$member"
            installed+=("$package")
        fi
        adb install --user 0 "$work/shared-$member.apk" >/dev/null
    }

    member_uid() {
        adb shell cmd package list packages --user "$1" -U "$2" | tr -d '\r' |
            awk -v package="package:$2" '$1 == package { sub(/^uid:/, "", $2); print $2 }'
    }

    assert_inventory() {
        local serial=$1 inventory command
        shift
        command='app eligibility'
        if [[ $serial != 0 ]]; then command='app profiles'; fi
        inventory=$(adb shell "$ABD $command --json")
        python3 -c '
import json, sys
inventory = json.load(sys.stdin)
serial, group, *members = sys.argv[1:]
if serial != "0":
    [inventory] = [row for row in inventory["profiles"] if row["serial"] == int(serial)]
[entry] = [row for row in inventory["groups"] if row["id"] == group]
assert entry["packages"] == sorted(members), entry
' "$serial" "$shared_id" "$@" <<<"$inventory"
    }

    assert_choice() {
        local serial=$1 mode=$2 paused=$3
        adb shell "$ABD app list --json" | python3 -c '
import json, sys
serial, group, mode, paused, *members = sys.argv[1:]
rows = [row for row in json.load(sys.stdin) if row.get("profile_serial", 0) == int(serial)]
assert all(row["package"] not in members for row in rows), rows
chosen = [row for row in rows if row["package"] == group]
if mode == "off":
    assert not chosen, chosen
else:
    [row] = chosen
    assert row["effective"]["http"]["level"] == mode, row
    assert (row.get("paused_until") is not None) == (paused == "yes"), row
' "$serial" "$group" "$mode" "$paused" "$first" "$second" "$third"
    }

    assert_response() {
        local user=$1 package=$2 address=$3 expected=$4 actual
        actual=$(fetch "$address" "$user" false "$package")
        [[ $actual == "$expected" ]] || die "shared UID probe for $package in Android user $user returned '$actual', expected '$expected'"
    }

    # With Host filtering the lookup of a blocked host is answered as
    # blocked, unless Android answers it from its cache: then the request
    # itself is refused.
    assert_host_blocked() {
        local user=$1 package=$2 actual
        actual=$(fetch "$blocked_host" "$user" false "$package")
        [[ $actual == "403 -" || $actual == "error lookup blocked" ]] ||
            die "shared UID host probe for $package in Android user $user returned '$actual'"
    }

    filtering_count() {
        adb shell "$ABD status --json" | python3 -c 'import json, sys; print(json.load(sys.stdin)["filtering_apps"])'
    }

    await_count() {
        local expected=$1 attempt
        for ((attempt = 0; attempt < 30; attempt++)); do
            [[ $(filtering_count) == "$expected" ]] && return 0
            sleep 1
        done
        die "the active app count did not reach $expected"
    }

    blocked_count() {
        adb shell "$ABD stats --json ${edges[*]}" | python3 -c '
import json, sys
rows = json.load(sys.stdin)["apps"]
assert all(member not in rows for member in sys.argv[2:]), rows.keys()
print(sum(sum(day) for day in rows.get(sys.argv[1], [])))
' "$group" "$first" "$second" "$third"
    }

    original_count=$(filtering_count)
    [[ $original_count =~ ^[0-9]+$ ]] || die "the active filtering count is unavailable"
    device_now=$(adb shell date +%s | tr -d '\r')
    [[ $device_now =~ ^[0-9]+$ ]] || die "could not read the device clock"
    today=$((device_now / 86400 * 86400))
    edges=()
    for ((day = -6; day <= 1; day++)); do edges+=("$((today + day * 86400))"); done
    before=$(blocked_count)

    install_member "$first"
    shared=$(member_uid 0 "$first")
    [[ $shared =~ ^[0-9]+$ && $shared != "$uid" ]] || die "unexpected UID for the shared group fixture"
    assert_inventory 0 "$first"
    adb shell "$ABD url-filtering off && $ABD app add $first"
    assert_choice 0 hosts no
    await_selected_uid "$shared" 0
    await_count "$((original_count + 1))"
    assert_host_blocked 0 "$first"

    install_member "$second"
    assert_host_blocked 0 "$second"
    [[ $(member_uid 0 "$second") == "$shared" ]] || die "Android did not assign the sibling to the same UID"
    assert_inventory 0 "$first" "$second"
    assert_choice 0 hosts no
    await_selected_uid "$shared" 0
    await_selected_uid "$uid" 0
    await_count "$((original_count + 1))"
    assert_host_blocked 0 "$first"
    echo "ok    an installed sibling inherits the active group selection and host filtering"

    # A blocked lookup counts each question it asks, A and AAAA, and a
    # request refused after a lookup Android answered from its cache counts
    # once more for that: three requests count three to nine times, and
    # only under the group.
    for ((attempt = 0; attempt < 45; attempt++)); do
        blocks=$(blocked_count)
        ((blocks <= before + 9)) || die "shared group statistics rose from $before to $blocks after three requests"
        ((blocks >= before + 3)) && break
        sleep 1
    done
    ((attempt < 45)) || die "shared group statistics rose from $before to $blocks after three requests"
    echo "ok    three blocked requests are counted under the shared group"

    adb shell "$ABD url-filtering on"
    assert_choice 0 full no
    adb shell "$ABD app pause $group --minutes 5"
    assert_choice 0 full yes
    await_selected_uid "$shared" 1
    await_count "$original_count"
    install_member "$third"
    assert_response 0 "$third" "$blocked_path" "404 -"
    [[ $(member_uid 0 "$third") == "$shared" ]] || die "Android did not assign the third member to the same UID"
    assert_inventory 0 "$first" "$second" "$third"
    assert_choice 0 full yes
    await_selected_uid "$shared" 1
    for package in "$first" "$second"; do assert_response 0 "$package" "$blocked_path" "404 -"; done
    adb shell "$ABD app resume $third"
    await_selected_uid "$shared" 0
    await_count "$((original_count + 1))"
    assert_choice 0 full no
    for package in "$first" "$second" "$third"; do assert_response 0 "$package" "$blocked_path" "403 -"; done
    echo "ok    the third member inherits the group's pause and uses Full mode when resumed"
    for package in "$first" "$third"; do
        tls=$(fetch https://example.com/blocked-by-check 0 false "$package")
        [[ $tls == "403 "*ADBlocker* ]] || die "group Full HTTPS probe for $package returned '$tls'"
    done
    echo "ok    Full HTTPS filtering works for both an original and a newly installed group member"

    adb uninstall "$third" >/dev/null
    adb shell "$ABD app remove $second"
    await_selected_uid "$shared" 1
    await_count "$original_count"
    install_member "$third"
    assert_response 0 "$third" "$blocked_path" "404 -"
    [[ $(member_uid 0 "$third") == "$shared" ]] || die "Android changed the reinstalled member's UID"
    assert_choice 0 off no
    for package in "$first" "$second"; do assert_response 0 "$package" "$blocked_path" "404 -"; done
    echo "ok    removing the group leaves both existing members and a reinstalled sibling unfiltered"

    created=$(adb shell pm create-user --profileOf 0 --managed ADBlockerSharedUID | tr -d '\r')
    group_user=$(sed -n 's/^Success: created user id \([1-9][0-9]*\)$/\1/p' <<<"$created")
    [[ $group_user =~ ^[1-9][0-9]*$ ]] || die "could not create the shared group work profile"
    adb shell am start-user -w "$group_user" >/dev/null
    adb shell cmd package install-existing --user "$group_user" "$first" >/dev/null
    serial=$(adb shell dumpsys user | tr -d '\r' | sed -n "s/.*UserInfo{${group_user}:[^}]*} serialNo=\([0-9][0-9]*\).*/\1/p")
    [[ $serial =~ ^[1-9][0-9]*$ ]] || die "no serial for the shared group work profile"
    group_work_key="$serial:$group"
    shared_work=$(member_uid "$group_user" "$first")
    [[ $shared_work =~ ^[0-9]+$ && $shared_work != "$shared" ]] || die "work and personal shared groups have the same UID"
    assert_inventory "$serial" "$first"
    adb shell "$ABD app add $group_work_key"
    await_selected_uid "$shared_work" 0
    await_count "$((original_count + 1))"
    assert_response "$group_user" "$first" "$blocked_path" "403 -"
    adb shell cmd package install-existing --user "$group_user" "$second" >/dev/null
    assert_response "$group_user" "$second" "$blocked_path" "403 -"
    [[ $(member_uid "$group_user" "$second") == "$shared_work" ]] || die "work siblings have different UIDs"
    assert_inventory "$serial" "$first" "$second"
    assert_inventory 0 "$first" "$second" "$third"
    await_selected_uid "$shared_work" 0
    await_count "$((original_count + 1))"
    assert_choice "$serial" full no
    assert_choice 0 off no
    assert_response 0 "$second" "$blocked_path" "404 -"
    adb shell "$ABD app add $group"
    await_selected_uid "$shared" 0
    await_count "$((original_count + 2))"
    assert_choice 0 full no
    assert_choice "$serial" full no
    assert_response 0 "$first" "$blocked_path" "403 -"
    assert_response "$group_user" "$first" "$blocked_path" "403 -"
    adb shell "$ABD app pause $group_work_key --minutes 5"
    await_selected_uid "$shared_work" 1
    await_selected_uid "$shared" 0
    await_count "$((original_count + 1))"
    assert_choice "$serial" full yes
    assert_choice 0 full no
    assert_response "$group_user" "$second" "$blocked_path" "404 -"
    assert_response 0 "$second" "$blocked_host" "403 -"
    echo "ok    the same shared ID has separate selection and pause state in work and personal"

    adb shell pm remove-user "$group_user" >/dev/null
    for ((attempt = 0; attempt < 60; attempt++)); do
        users=$(adb shell pm list users)
        [[ $users != *"UserInfo{$group_user:"* ]] && break
        sleep 1
    done
    ((attempt < 60)) || die "the shared group work profile was not removed"
    group_user=
    for ((attempt = 0; attempt < 30; attempt++)); do
        retained=$(adb shell "for path in $DATA/apps $DATA/pauses; do
            if [ -f \$path ]; then grep -F '$group_work_key' \$path; fi
        done; echo end")
        [[ $retained == end ]] && break
        sleep 1
    done
    [[ $retained == end ]] || die "the deleted profile still has a stored group selection or pause"
    assert_choice "$serial" off no
    assert_choice 0 full no
    await_selected_uid "$shared" 0
    assert_response 0 "$first" "$blocked_host" "403 -"
    echo "ok    deleting the work group profile clears its stored choice and pause while personal stays filtered"

    for package in "$first" "$second" "$third"; do adb uninstall "$package" >/dev/null; done
    await_selected_uid "$shared" 1
    await_selected_uid "$uid" 0
    await_count "$original_count"
    adb shell "$ABD app remove $group"
    assert_choice 0 off no
    echo "ok    uninstalling every group member removes the obsolete UID from capture"
)
