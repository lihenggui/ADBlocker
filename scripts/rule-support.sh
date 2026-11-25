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

set -euo pipefail

if [ "$#" -eq 0 ]; then
    echo "Usage: $0 LIST..." >&2
    exit 2
fi

abd=${ABD:-target/debug/abd}
data=$(mktemp -d)
trap 'rmdir -- "$data"' EXIT
for list in "$@"; do
    printf '%s\n' "$list"
    ABD_DATA="$data" "$abd" rules check --json < "$list" | jq -r '
        def directive: .reason == "include-not-followed" or .reason == "malformed-directive";
        .rules as $rules
        | .issues as $issues
        | ($issues | map(select(directive | not)) | length) as $unapplied
        | "  Rules: \($rules)", "  Applied: \($rules - $unapplied)", "  Unapplied: \($unapplied)",
          ($issues | group_by([.kind, .reason])
                   | map({ count: length, kind: .[0].kind, reason: .[0].reason, directive: (.[0] | directive) })
                   | sort_by(-.count, .kind, .reason)[]
                   | "  \(.count)	\(.kind)	\(.reason)\(if .directive then " (directive)" else "" end)")
    '
done
