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

active_rule_count() {
    adb shell "$1 status --json" | jq -er '.rules | numbers'
}

# The daemon publishes this count after installing compiled filters. A known
# fixture delta identifies the reload when the other rule inputs are stable.
# The count cannot distinguish generations with the same total.
await_rule_count() {
    local daemon=$1 expected=$2 actual deadline=$((SECONDS + 60))
    while :; do
        if actual=$(active_rule_count "$daemon") && [[ $actual == "$expected" ]]; then
            return 0
        fi
        if ((SECONDS >= deadline)); then
            echo "The daemon reported ${actual:-no} active rules, expected $expected" >&2
            return 1
        fi
        sleep .5
    done
}
