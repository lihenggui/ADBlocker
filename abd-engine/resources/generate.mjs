// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Mercury Li
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 as
// published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

// Regenerate with Node.js 22+ after verifying ublock/SHA256SUMS.
// Modern uBO registers functions and metadata in ES modules. adblock-rust's
// legacy /// assembler discards dependencies and trust markers.
import { builtinScriptlets } from './ublock/js/resources/scriptlets.js';
import { writeFileSync } from 'node:fs';

const entries = new Map();
for (const entry of builtinScriptlets) {
    if (entries.has(entry.name)) throw new Error(`Duplicate resource ${entry.name}`);
    entries.set(entry.name, entry);
}

// Dependencies of public scriptlets must never grant additional authority.
function privileged(entry, seen = new Set()) {
    if (seen.has(entry.name)) throw new Error(`Circular dependency: ${entry.name}`);
    if (entry.requiresTrust || [entry.name, ...(entry.aliases ?? [])].some(name => name.startsWith('trusted-'))) {
        return true;
    }
    return (entry.dependencies ?? []).some(name => {
        const dependency = entries.get(name);
        if (!dependency) throw new Error(`Missing dependency: ${name}`);
        return privileged(dependency, new Set([...seen, entry.name]));
    });
}

const resources = Array.from(entries.values(), entry => {
    const { name, aliases = [], dependencies = [], fn } = entry;
    const code = fn.toString();
    const helper = name.endsWith('.fn');
    const callable = /^function [A-Za-z_$][\w$]*\(/.test(code);
    if ((!callable && !(helper && /^class \w+\s*\{/.test(code))) || code.includes('{{1}}')) {
        throw new Error(`Unsupported scriptlet format: ${name}`);
    }
    const kind = helper ? 'fn/javascript' : 'application/javascript';
    return {
        name,
        aliases,
        kind: { mime: kind },
        content: Buffer.from(code).toString('base64'),
        dependencies,
        ...(privileged(entry) ? { permission: 1 } : {}),
    };
});
writeFileSync(new URL('./scriptlets.json', import.meta.url), JSON.stringify(resources) + '\n');
