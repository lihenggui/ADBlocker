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
cd "$(dirname "$0")/.."
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/module/bin" "$fixture/tools"
# The module the app may have installed is looked for under the fixture too.
sed -e 's|^DATA=/data/adb/adblocker$|DATA=$FIXTURE/data|' -e 's|/data/adb/|$FIXTURE/adb/|g' \
    module/uninstall.sh >"$fixture/module/uninstall.sh"
export FIXTURE="$fixture"

cat >"$fixture/module/bin/abd" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$FIXTURE/calls"
case "$*" in
    module-stop) exit "$STOP_STATUS" ;;
    unhook) exit "$UNHOOK_STATUS" ;;
    'trust remove')
        test "${ABD_KEEP_TRUST_STORES:-0}" = "$EXPECTED_KEEP" || touch "$FIXTURE/wrong-keep"
        exit "$TRUST_STATUS" ;;
    *) exit 99 ;;
esac
EOF
chmod +x "$fixture/module/bin/abd"

cat >"$fixture/tools/rm" <<'EOF'
#!/bin/sh
case "$*" in
    *'*'*) touch "$FIXTURE/unexpanded-glob"; exit 1 ;;
esac
exec /bin/rm "$@"
EOF
chmod +x "$fixture/tools/rm"

check_uninstall() {
    export STOP_STATUS=$1 UNHOOK_STATUS=$2 TRUST_STATUS=$3
    EXPECTED_KEEP=0
    [ "$1" -eq 0 ] || EXPECTED_KEEP=1
    export EXPECTED_KEEP
    rm -rf "$fixture/data"
    mkdir -p "$fixture/data/ca" "$fixture/data/store" "$fixture/data/user-store"
    touch "$fixture/data/ca/user.key" "$fixture/data/store/system.0" \
        "$fixture/data/user-store/user.0" "$fixture/data/apps" "$fixture/data/.private" \
        "$fixture/data/..private"
    ln -s absent "$fixture/data/..dangling"
    : >"$fixture/calls"
    (cd "$fixture/module" && sh uninstall.sh) 2>"$fixture/said"
    test ! -e "$fixture/wrong-keep"
    printf '%s\n' module-stop unhook 'trust remove' | cmp - "$fixture/calls"
    test ! -e "$fixture/data/ca"
    test ! -e "$fixture/data/apps"
    test ! -e "$fixture/data/.private"
    test ! -e "$fixture/data/..private"
    test ! -L "$fixture/data/..dangling"
    if [ "$4" = keep ]; then
        test -f "$fixture/data/store/system.0"
        test -f "$fixture/data/user-store/user.0"
        (cd "$fixture/module" && PATH="$fixture/tools:$PATH" sh uninstall.sh) 2>/dev/null
        test ! -e "$fixture/unexpanded-glob"
    else
        test ! -e "$fixture/data"
    fi
    # One line for the root manager's log for each step that did not finish.
    lines=0
    for said in $5; do
        case "$said" in
        unhook) grep -q 'abd unhook failed' "$fixture/said" ;;
        kept) grep -q 'store copies are kept' "$fixture/said" ;;
        failed) grep -q 'abd trust remove failed' "$fixture/said" ;;
        esac
        lines=$((lines + 1))
    done
    test "$(($(wc -l <"$fixture/said")))" -eq "$lines"
}

check_uninstall 0 0 0 remove ''
check_uninstall 0 1 3 keep 'unhook kept'
check_uninstall 1 1 1 keep 'unhook failed'
check_uninstall 1 0 0 keep 'kept'
