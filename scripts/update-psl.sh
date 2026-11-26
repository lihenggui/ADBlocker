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

if [[ $# -gt 1 || ( $# -eq 1 && ( -z $1 || $1 == -* ) ) ]]; then
    printf 'Usage: %s [VERSION]\n' "$0" >&2
    exit 2
fi

CDPATH='' cd -- "$(dirname -- "$0")/.."
if [[ $# -eq 1 ]]; then
    cargo update --quiet -p psl --precise "$1"
fi

python3 -I - <<'PY'
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys


def replace_once(content, anchor, value, label):
    updated, count = re.subn(
        anchor, lambda match: f"{match[1]}{value}{match[2]}", content, flags=re.MULTILINE
    )
    if count != 1:
        sys.exit(f"error: {label}: expected exactly one anchor, found {count}")
    return updated


try:
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--quiet", "--format-version", "1", "--locked"]
        )
    )
except subprocess.CalledProcessError as error:
    sys.exit(error.returncode)

packages = [package for package in metadata["packages"] if package["name"] == "psl"]
if len(packages) != 1:
    sys.exit(f"error: Cargo metadata: expected exactly one locked psl package, found {len(packages)}")
package = packages[0]
if not (package["source"] or "").startswith("registry+"):
    sys.exit("error: Cargo metadata: expected psl to come from a Cargo registry")

version = package["version"]
rules = (Path(package["manifest_path"]).parent / "data/rules.txt").read_bytes()
digest = hashlib.sha256(rules).hexdigest()

source_path = Path("abd-engine/src/public_suffix.rs")
notices_path = Path("THIRD_PARTY_NOTICES.md")
rules_path = Path("abd-engine/resources/psl/rules.txt")
source = source_path.read_bytes()
notices = notices_path.read_bytes()

# A stale anchor must leave the bundled files untouched.
updated_source = replace_once(
    source.decode("utf-8"), r'^(const RULES_OF_PSL: &str = ")[^"\n]+(";)$',
    version, f"{source_path} (RULES_OF_PSL)",
)
prose_prefix = r"  as \[psl "
prose_suffix = r"\]\[psl-crate\] carries it in `data/rules\.txt`, SHA-256"
updated_notices = replace_once(
    notices.decode("utf-8"),
    rf"^({prose_prefix})[^\]\n]+({prose_suffix})$",
    version, f"{notices_path} (psl version in prose)",
)
updated_notices = replace_once(
    updated_notices,
    rf"^({prose_prefix}[^\]\n]+{prose_suffix}\n  `)[0-9a-f]{{64}}(`\.)$",
    digest, f"{notices_path} (psl SHA-256)",
)
updated_notices = replace_once(
    updated_notices,
    r"^(\[psl-crate\]: https://github\.com/addr-rs/psl/blob/v)[^/\n]+(/data/rules\.txt)$",
    version, f"{notices_path} ([psl-crate] tag)",
)

current_rules = rules_path.read_bytes() if rules_path.exists() else None
for path, current, updated in (
    (rules_path, current_rules, rules),
    (source_path, source, updated_source.encode("utf-8")),
    (notices_path, notices, updated_notices.encode("utf-8")),
):
    if current != updated:
        path.write_bytes(updated)

print(f"Synced Public Suffix List from psl {version}")
PY
