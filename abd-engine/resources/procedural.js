/*******************************************************************************

    uBlock Origin - a comprehensive, efficient content blocker
    Copyright (C) 2014-present Raymond Hill

    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation, either version 3 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License
    along with this program.  If not, see {http://www.gnu.org/licenses/}.

    Home: https://github.com/gorhill/uBlock
*/

// Copyright (C) 2026 Mercury Li
// Adapted for the adblock-rust selector/action format and the page's main
// world. ADBlocker supplies the JSON literal as the argument.
(function (actions) {
    'use strict';

    const limit = 256;
    const maxText = 1024;
    const maxPassMs = 12;
    const throttleMs = 100;
    const hide = [['display', 'none']];
    const now = performance.now.bind(performance);
    const bounded = Symbol();
    const slow = Symbol();
    const select = Function.call.bind(Document.prototype.querySelectorAll);
    const selectIn = Function.call.bind(Element.prototype.querySelectorAll);
    const parent = Function.call.bind(Element.prototype.closest);
    const matches = Function.call.bind(Element.prototype.matches);
    const computedStyle = getComputedStyle.bind(window);

    function unquote(raw) {
        const quote = raw[0];
        if ((quote !== '"' && quote !== "'") || raw.at(-1) !== quote) return raw;
        return raw.slice(1, -1).replace(/\\(.)/g, (escape, char) =>
            char === quote || char === '\\' ? char : escape
        );
    }

    function pattern(raw, exact = false) {
        raw = raw.trim();
        const literal = /^\/(.+)\/([imu]*)$/.exec(raw);
        if (literal === null) {
            const plain = unquote(raw).replace(/[.*+?^\${}()|[\]\\]/g, '\\$&');
            return new RegExp(exact && plain ? `^${plain}$` : plain);
        }
        // A page can supply the text a list's pattern runs against. Keep
        // backtracking work small even for a deliberately hostile string.
        const code = literal[1].replace(/\\[^1-9k]|\[(?:\\.|[^\]\\])*\]/g, '_');
        if (code.length > 160 || /\\|\(\?|\)[*+?]|[{}|]/.test(code)
            || (code.match(/[*+?]/g) || []).length > 1) {
            throw bounded;
        }
        return new RegExp(literal[1], literal[2]);
    }

    function text(node) {
        const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
        let data = '';
        for (let i = 0; i < limit && walker.nextNode() !== null; i++) {
            data += walker.currentNode.substringData(0, maxText - data.length);
            if (data.length >= maxText) break;
        }
        return data;
    }

    function upward(arg) {
        if (!/^\d+$/.test(arg)) return node => node.parentElement && parent(node.parentElement, arg);
        const count = Number(arg);
        if (count < 1 || count > 32) throw bounded;
        return node => {
            for (let i = 0; node && i < count; i++) node = node.parentElement;
            return node;
        };
    }

    function query(node, arg) {
        if (node === document) return select(document, arg);
        if (/^\s*[+~]/.test(arg)) {
            const scope = node.parentElement;
            if (!scope) return [];
            let pos = 1;
            while ((node = node.previousElementSibling) !== null && ++pos <= limit) {}
            if (pos > limit) throw bounded;
            return selectIn(scope, `:scope > :nth-child(${pos})${arg}`);
        }
        if (/^\s*>/.test(arg)) return selectIn(node, `:scope ${arg}`);
        if (/^\s/.test(arg)) return selectIn(node, arg);
        return matches(node, arg) ? [node] : [];
    }

    function predicate(operator, arg) {
        switch (operator) {
        case 'has-text': {
            const re = pattern(arg);
            return node => node.nodeType === 1 && re.test(text(node));
        }
        case 'matches-attr': {
            const [name, value = ''] = arg.split(/=(.*)/s);
            const attr = pattern(name.trim(), true);
            const val = pattern(value.trim(), true);
            return node => node.attributes && Array.prototype.some.call(
                Array.prototype.slice.call(node.attributes, 0, 32),
                item => attr.test(item.name) && val.test(item.value.slice(0, maxText))
            );
        }
        case 'matches-css': case 'matches-css-before': case 'matches-css-after': {
            const [name, value] = arg.split(/:(.*)/s);
            const [pseudoOrProperty, property = pseudoOrProperty] = name.split(',').map(part => part.trim());
            const re = pattern(value, true);
            const pseudo = operator !== 'matches-css' ? operator.slice(12)
                : name.includes(',') ? pseudoOrProperty : '';
            if (pseudo && !['before', 'after'].includes(pseudo)) throw bounded;
            return node => node.nodeType === 1 && re.test(
                computedStyle(node, pseudo ? `::${pseudo}` : null).getPropertyValue(property).slice(0, maxText)
            );
        }
        case 'matches-path': {
            const re = pattern(arg.replace(/\P{ASCII}/gu, encodeURIComponent));
            return () => re.test((location.pathname + location.search).slice(0, maxText));
        }
        case 'min-text-length': {
            const n = Number(arg);
            return node => node.nodeType === 1 && n > 0 && text(node).length >= n;
        }
        default:
            throw bounded;
        }
    }

    function task({type, arg}) {
        switch (type) {
        case 'css-selector':
            return (node, add) => {
                const nodes = query(node, arg);
                if (nodes.length > limit) throw bounded;
                for (const match of nodes) add(match);
            };
        case 'upward': {
            const find = upward(arg.trim());
            return (node, add) => add(find(node));
        }
        case 'xpath': {
            const expression = document.createExpression(arg, null);
            return (node, add) => {
                const items = expression.evaluate(node, XPathResult.UNORDERED_NODE_ITERATOR_TYPE, null);
                for (let item = items.iterateNext(); item; item = items.iterateNext()) add(item);
            };
        }
        default: {
            const test = predicate(type, arg);
            return (node, add) => { if (test(node)) add(node); };
        }
        }
    }

    function declarations(arg) {
        const parts = arg.split(';').filter(part => part.trim());
        if (!parts.length || parts.length > 16) throw bounded;
        return parts.map(part => {
            const [name, raw] = part.split(/:(.*)/s);
            const property = name.trim().toLowerCase();
            const value = raw.trim().replace(/\s*!important$/i, '');
            // Only declarations that cannot name a resource. CSS escapes,
            // custom properties, and URL-capable functions are omitted.
            if (!/^[a-z][a-z-]*$/.test(property)
                || !/^[a-z0-9 .,%+#()/-]+$/i.test(value)
                || value.match(/[a-z-]+\s*\(/gi)?.some(
                    fn => !/^(rgb|rgba|hsl|hsla|calc|min|max|clamp|translate[x-y]?|scale[x-y]?|rotate)\(/i.test(fn)
                )) {
                throw bounded;
            }
            return [property, value];
        });
    }

    const rules = [];
    for (const {selector, action} of actions) {
        try {
            const tasks = selector.map(task);
            if (!tasks.length || tasks.length > 16) continue;
            const style = !action ? hide : action.type === 'style' ? declarations(action.arg) : null;
            rules.push({ tasks, action, style, disabled: false });
        } catch {}
    }
    if (!rules.length) return;

    // Only properties touched by the runtime are remembered. The page can
    // change other properties and can reuse an element after it stops matching.
    let applied = new Map();
    function applyStyles(matches) {
        for (const [node, properties] of applied) {
            for (const [name, [value, priority, written]] of properties) {
                if (matches.get(node)?.has(name)) continue;
                if (node.style.getPropertyValue(name) === written
                    && node.style.getPropertyPriority(name) === 'important') {
                    node.style.setProperty(name, value, priority);
                }
                properties.delete(name);
            }
            if (!properties.size) applied.delete(node);
        }
        for (const [node, wanted] of matches) {
            const properties = applied.get(node) || new Map();
            for (const [name, value] of wanted) {
                let previous = properties.get(name);
                const original = node.style.getPropertyValue(name);
                const priority = node.style.getPropertyPriority(name);
                if (!previous || original !== previous[2] || priority !== 'important') {
                    previous = [original, priority, ''];
                }
                if (original !== value || priority !== 'important') {
                    node.style.setProperty(name, value, 'important');
                }
                previous[2] = node.style.getPropertyValue(name);
                properties.set(name, previous);
            }
            applied.set(node, properties);
        }
    }

    // Too much work for one rule disables it for good. Running out of time
    // only ends this pass: a busy main thread while a page loads is no
    // reason to stop hiding what the rule matches.
    function execute(rule, deadline) {
        let nodes = new Set([document]);
        let work = 0;
        const step = () => {
            if (++work > limit) throw bounded;
            if (now() > deadline) throw slow;
        };
        for (const run of rule.tasks) {
            const next = new Set();
            const add = node => {
                step();
                if (node === document || node?.nodeType === 1) next.add(node);
            };
            for (const node of nodes) {
                step();
                run(node, add);
            }
            if (now() > deadline) throw slow;
            nodes = next;
            if (!nodes.size) break;
        }
        return nodes;
    }

    let timer = 0;
    let observing = false;
    let cursor = 0;
    let retry = false;
    const schedule = () => {
        if (observing && !timer) timer = setTimeout(update, throttleMs);
    };
    const observer = new MutationObserver(schedule);
    const options = { subtree: true, childList: true, attributes: true, characterData: true };
    const hits = new Map();
    function update() {
        timer = 0;
        const deadline = now() + maxPassMs;
        while (cursor < rules.length && now() < deadline) {
            const rule = rules[cursor++];
            if (rule.disabled) continue;
            try {
                rule.last = execute(rule, Math.min(deadline, now() + 5));
            } catch (error) {
                if (error !== slow) {
                    rule.disabled = true;
                    rule.last = null;
                    continue;
                }
                // Keep what it matched before, so nothing hidden reappears,
                // and try it again on the next pass.
                retry = true;
            }
            if (rule.last) hits.set(rule, rule.last);
        }
        if (cursor < rules.length) {
            schedule();
            return;
        }
        cursor = 0;
        observer.disconnect();
        try {
            const styles = new Map();
            let effects = 0;
            for (const [rule, nodes] of hits) {
                for (const node of nodes) {
                    if (++effects > limit) break;
                    if (node.nodeType !== 1 || !node.isConnected) continue;
                    if (rule.style && node.style) {
                        const properties = styles.get(node) || new Map();
                        for (const [name, value] of rule.style) properties.set(name, value);
                        styles.set(node, properties);
                    } else if (rule.action && node !== document.documentElement
                        && node !== document.head && node !== document.body) {
                        const {type, arg} = rule.action;
                        if (type === 'remove') node.remove();
                        if (type === 'remove-attr' && /^[a-z_][\w:-]*$/i.test(arg)) node.removeAttribute(arg);
                        if (type === 'remove-class' && arg && !/\s/.test(arg)) node.classList.remove(arg);
                    }
                }
                if (effects > limit) break;
            }
            applyStyles(styles);
        } finally {
            hits.clear();
            if (observing) observer.observe(document, options);
            if (retry) {
                retry = false;
                schedule();
            }
        }
    }

    // The first pass runs as soon as the document is parsed, so that what a
    // rule hides is not shown first; later passes follow changes, throttled.
    function start() {
        if (observing) return;
        observing = true;
        observer.observe(document, options);
        update();
    }
    addEventListener('pagehide', () => {
        observing = false;
        observer.disconnect();
        clearTimeout(timer);
        timer = cursor = 0;
        retry = false;
        hits.clear();
    });
    addEventListener('pageshow', event => { if (event.persisted) start(); });
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', start, { once: true });
    } else {
        start();
    }
})
