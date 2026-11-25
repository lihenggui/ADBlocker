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

// Regenerate with Node.js 22+. Seven image and media inputs are stored
// as base64 so the F-Droid source tree contains no new binary files.
// SHA256SUMS pins their decoded upstream bytes.
// Modern uBO registers functions and metadata in ES modules. adblock-rust's
// legacy /// assembler discards dependencies and trust markers.
import { builtinScriptlets } from './ublock/js/resources/scriptlets.js';
import redirectResources from './ublock/js/redirect-resources.js';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { extname } from 'node:path';

const upstream = new URL('./ublock/', import.meta.url);
const redirectDirectory = 'web_accessible_resources/';
const encodedExtensions = new Set(['.gif', '.png', '.mp3', '.mp4']);
const redirectBodies = new Map();
for (const entry of readFileSync(new URL('SHA256SUMS', upstream), 'utf8').trimEnd().split('\n')) {
    const [expected, path] = entry.split('  ');
    const redirect = path.startsWith(redirectDirectory);
    const encoded = redirect && encodedExtensions.has(extname(path));
    const contents = readFileSync(new URL(encoded ? `${path}.base64` : path, upstream));
    const bytes = encoded ? Buffer.from(contents.toString('ascii'), 'base64') : contents;
    const actual = createHash('sha256').update(bytes).digest('hex');
    if (actual !== expected) {
        throw new Error(`SHA-256 mismatch for ${path}: expected ${expected}, got ${actual}`);
    }
    if (redirect) redirectBodies.set(path.slice(redirectDirectory.length), bytes);
}

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

// AdguardTeam/Scriptlets at 420ffa66c6e713ffa95c80e4bfa356b1a796c3a3,
// wiki/compatibility-table.md (Redirects), checked against src/redirects/.
const adguardRedirects = new Map(Object.entries({
    '1x1-transparent.gif': '1x1.gif',
    '2x2-transparent.png': '2x2.png',
    '3x2-transparent.png': '3x2.png',
    '32x32-transparent.png': '32x32.png',
    'amazon-apstag': 'amazon_apstag.js',
    'empty': 'empty',
    'fingerprintjs2': 'fingerprint2.js',
    'fingerprintjs3': 'fingerprint3.js',
    'google-analytics': 'google-analytics_analytics.js',
    'google-analytics-ga': 'google-analytics_ga.js',
    'google-ima3': 'google-ima.js',
    'googlesyndication-adsbygoogle': 'googlesyndication_adsbygoogle.js',
    'googletagservices-gpt': 'googletagservices_gpt.js',
    'noeval': 'noeval-silent.js',
    'noopcss': 'noop.css',
    'noopframe': 'noop.html',
    'noopjs': 'noop.js',
    'noopjson': 'noop.json',
    'nooptext': 'noop.txt',
    'noopmp3-0.1s': 'noop-0.1s.mp3',
    'noopmp4-1s': 'noop-1s.mp4',
    'noopvmap-1.0': 'noop-vmap1.xml',
    'noopvast-2.0': 'noop-vast2.xml',
    'noopvast-3.0': 'noop-vast3.xml',
    'noopvast-4.0': 'noop-vast4.xml',
    'prebid-ads': 'prebid-ads.js',
    'prevent-bab': 'nobab.js',
    'prevent-bab2': 'nobab2.js',
    'prevent-fab-3.2.0': 'nofab.js',
    'prevent-popads-net': 'popads.js',
    'scorecardresearch-beacon': 'scorecardresearch_beacon.js',
    'set-popads-dummy': 'popads-dummy.js',
}));

const mimeTypes = {
    '.css': 'text/css',
    '.gif': 'image/gif',
    '.html': 'text/html',
    '.js': 'application/javascript',
    '.json': 'application/json',
    '.mp3': 'audio/mp3',
    '.mp4': 'video/mp4',
    '.png': 'image/png',
    '.txt': 'text/plain',
    '.xml': 'text/xml',
};

const redirects = new Map();
const redirectNames = new Map();
function addRedirectName(alias, name) {
    const existing = redirectNames.get(alias);
    if (existing !== undefined && existing !== name) {
        throw new Error(`Conflicting redirect name ${alias}: ${existing}, ${name}`);
    }
    redirectNames.set(alias, name);
    if (alias !== name && !redirects.get(name).aliases.includes(alias)) {
        redirects.get(name).aliases.push(alias);
    }
}

for (const [name, entry] of redirectResources) {
    // The extension would supply the original frame URL and handle clicks.
    // The sandboxed response would leave an unusable external-link control.
    if (name === 'click2load.html') continue;
    const mime = name === 'empty' ? 'text/plain' : mimeTypes[extname(name)];
    if (!mime) throw new Error(`Unknown MIME type for ${name}`);
    const content = redirectBodies.get(name);
    if (!content) throw new Error(`Missing SHA-256 entry for redirect resource ${name}`);
    redirects.set(name, { name, aliases: [], kind: { mime }, content: content.toString('base64') });
    for (const alias of [name, ...[].concat(entry.alias ?? [])]) {
        addRedirectName(alias, name);
    }
}
for (const [alias, name] of adguardRedirects) {
    if (!redirects.has(name)) throw new Error(`Missing redirect resource ${name} for ${alias}`);
    addRedirectName(alias, name);
}
writeFileSync(new URL('./redirects.json', import.meta.url), JSON.stringify([...redirects.values()]) + '\n');
