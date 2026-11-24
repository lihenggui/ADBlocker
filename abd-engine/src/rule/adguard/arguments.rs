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

//! Translate positional arguments to the bundled implementations' conventions.

pub(super) fn adapt(name: &str, resource: &str, mut args: Vec<String>) -> Option<Vec<String>> {
    match resource {
        "set-constant" => set_constant(name, &mut args)?,
        "trusted-set-constant" => trusted_constant(&mut args)?,
        "prevent-window-open" => window_open(&mut args)?,
        "prevent-fetch" | "prevent-xhr" => prevent_request(resource, &mut args)?,
        "json-prune"
        | "evaldata-prune"
        | "json-prune-fetch-response"
        | "json-prune-xhr-response" => prune(name, resource, &mut args)?,
        "addEventListener-defuser" | "remove-attr" | "remove-class" => {
            element_arguments(resource, &mut args)?;
        }
        "trusted-replace-node-text"
        | "trusted-replace-argument"
        | "trusted-prune-inbound-object"
        | "trusted-replace-outbound-text" => trusted_replacement(resource, &mut args)?,
        "set-cookie" | "set-cookie-reload" | "trusted-set-cookie" | "trusted-set-cookie-reload" => {
            cookie(resource, &mut args)?;
        }
        "href-sanitizer" => href(&mut args)?,
        // AdGuard's delay selects when to stop; the bundled scriptlet uses
        // a supplied delay as a switch to stop immediately.
        "prevent-refresh" => return None,
        // json-edit aliases in AdGuard are prune calls. The bundled editor
        // accepts a different language and does not implement all prune paths.
        "json-edit" | "json-edit-fetch-response" | "json-edit-xhr-response" => return None,
        _ => passthrough(resource, &args)?,
    }
    Some(args)
}

fn at(args: &[String], index: usize) -> &str {
    args.get(index).map(String::as_str).unwrap_or_default()
}

fn set_constant(name: &str, args: &mut Vec<String>) -> Option<()> {
    let ubo_alias = name.starts_with("ubo-") || name.ends_with(".js");
    let wrapper_index = if ubo_alias { 2 } else { 3 };
    let wrapper = match at(args, wrapper_index) {
        "" => None,
        "asFunction" => Some("function"),
        "asCallback" => Some("callback"),
        "asResolved" => Some("resolved"),
        "asRejected" => Some("rejected"),
        _ => return None,
    };
    if (!ubo_alias && !at(args, 2).is_empty()) || args.len() > wrapper_index + 1 {
        return None;
    }
    let value = args.get_mut(1)?;
    match value.as_str() {
        "undefined" | "false" | "true" | "null" | "" | "emptyArr" | "emptyObj" | "noopFunc"
        | "trueFunc" | "falseFunc" | "throwFunc" | "-1" => (),
        other if other.bytes().all(|b| b.is_ascii_digit()) && other.parse::<i16>().is_ok() => (),
        _ => return None,
    }
    if let Some(wrapper) = wrapper {
        args.truncate(2);
        args.extend(["as".to_owned(), wrapper.to_owned()]);
    } else if args.len() > 2 {
        args.truncate(2);
    }
    Some(())
}

fn trusted_constant(args: &mut Vec<String>) -> Option<()> {
    if args.len() > 3 || !at(args, 2).is_empty() {
        return None;
    }
    args.truncate(2);
    let value = args.get_mut(1)?;
    if matches!(value.as_str(), "undefined" | "false" | "true" | "null") {
        return Some(());
    }
    // AdGuard infers values and falls back to a string. Regex, NaN and
    // ambiguous coercions cannot be expressed through the JSON directive.
    if value == "NaN" || (value.starts_with('/') && value.ends_with('/')) {
        return None;
    }
    let json = match serde_json::from_str::<serde_json::Value>(value) {
        Ok(parsed) if parsed.is_number() => {
            (parsed.as_f64()?.abs() <= f64::from(i16::MAX)).then_some(parsed)?
        }
        Ok(parsed) => parsed,
        Err(_) if value.is_empty() => serde_json::Value::from(0),
        Err(_)
            if value.trim() != value
                || value.parse::<f64>().is_ok()
                || ["0x", "0b", "0o"]
                    .iter()
                    .any(|prefix| value.starts_with(prefix)) =>
        {
            return None;
        }
        Err(_) => serde_json::Value::String(value.clone()),
    };
    *value = format!("json:{json}");
    Some(())
}

fn window_open(args: &mut [String]) -> Option<()> {
    let pattern = at(args, 0);
    if args.len() > 3 || matches!(pattern, "0" | "1" | "debug") {
        return None;
    }
    if !matches!(at(args, 2), "" | "obj" | "iframe") {
        return None;
    }
    let delay = at(args, 1);
    if !delay.is_empty() && delay.parse::<i32>().is_err() {
        return None;
    }
    if pattern == "*" {
        args[0].clear();
    }
    Some(())
}

fn prevent_request(resource: &str, args: &mut Vec<String>) -> Option<()> {
    let pattern = request_pattern(args.first()?)?;
    if resource == "prevent-fetch" {
        // The bundled fetch helper accepts fixed types but the AdGuard
        // configuration can set status, redirected and other properties.
        if args.len() > 3 || !at(args, 2).is_empty() {
            return None;
        }
        let body = match at(args, 1) {
            "" | "emptyObj" => "emptyObj",
            body @ ("emptyArr" | "emptyStr") => body,
            _ => return None,
        };
        *args = vec![pattern, body.to_owned()];
    } else {
        if args.len() > 2
            || pattern
                .split_once(':')
                .is_some_and(|(key, _)| !matches!(key, "url" | "method"))
            || !matches!(at(args, 1), "" | "emptyObj" | "emptyArr" | "emptyStr")
        {
            return None;
        }
        args[0] = pattern;
    }
    Some(())
}

fn request_pattern(pattern: &str) -> Option<String> {
    if matches!(pattern, "" | "*") {
        return Some("/^/".to_owned());
    }
    // uBO splits each token at ':' and silently ignores missing request
    // fields. Keep the subset for which neither behavior broadens the match.
    let value = match pattern.split_once(':') {
        Some(("url" | "method" | "mode" | "credentials" | "cache" | "redirect", value)) => value,
        Some(_) => return None,
        None => pattern,
    };
    (!value.is_empty()
        && !value.starts_with('!')
        && !value.contains(':')
        && !pattern.contains(char::is_whitespace))
    .then(|| pattern.to_owned())
}

fn prune(name: &str, resource: &str, args: &mut Vec<String>) -> Option<()> {
    let response = matches!(
        resource,
        "json-prune-fetch-response" | "json-prune-xhr-response"
    );
    // Keep common legacy property paths. AdGuard's JSONPath and value
    // qualifiers can change which objects are pruned.
    for path in args.iter().take(2) {
        if path.contains('$')
            || path.contains(".[-]")
            || path.contains(".{-}")
            || path.contains(".[=]")
            || path.contains(".{=}")
        {
            return None;
        }
    }
    if !response {
        let slots = if resource == "evaldata-prune" { 2 } else { 3 };
        if args.iter().skip(slots).any(|arg| !arg.is_empty()) {
            return None;
        }
        args.truncate(slots);
        return Some(());
    }
    if name.starts_with("ubo-") {
        // Alias calls may use uBO key/value options. Positional AdGuard
        // options cannot safely be distinguished from these.
        return (args.len() <= 2).then_some(());
    }
    if args.len() > 5 || !at(args, 4).is_empty() {
        return None;
    }
    let props = at(args, 2);
    let stack = at(args, 3).to_owned();
    // The response helper ignores absent fields, so restrict this condition
    // to an ordinary URL expression.
    let url = props.strip_prefix("url:").unwrap_or(props);
    let props = if props.is_empty() {
        None
    } else {
        if props != url && url.is_empty()
            || url.contains(':')
            || matches!(url, "*" | "")
            || url.starts_with('!')
            || url.contains(char::is_whitespace)
        {
            return None;
        }
        Some(props.to_owned())
    };
    args.truncate(2);
    if let Some(props) = props {
        args.extend(["propsToMatch".to_owned(), props]);
    }
    if !stack.is_empty() {
        args.extend(["stackToMatch".to_owned(), stack]);
    }
    Some(())
}

fn element_arguments(resource: &str, args: &mut Vec<String>) -> Option<()> {
    if resource == "addEventListener-defuser" {
        return (args.len() <= 2
            || (args.len() == 4 && args[2] == "elements" && !args[3].is_empty()))
        .then_some(());
    }
    let token = at(args, 0);
    let selector = at(args, 1);
    let behavior = at(args, 2);
    if args.len() > 3
        || token.is_empty()
        || !behavior
            .split_ascii_whitespace()
            .all(|flag| matches!(flag, "asap" | "stay" | "complete"))
        || (args.len() >= 3 && !behavior.split_ascii_whitespace().any(|flag| flag == "stay"))
        || (resource == "remove-class" && selector.contains(','))
    {
        return None;
    }
    if args.len() < 3 {
        args.resize(2, String::new());
        args.push("asap stay".to_owned());
    }
    Some(())
}

fn cookie(resource: &str, args: &mut [String]) -> Option<()> {
    let trusted = resource.starts_with("trusted-");
    let path = if trusted { 3 } else { 2 };
    if args.len() < 2
        || args.len() > path + 2
        || args
            .get(path)
            .is_some_and(|path| !matches!(path.as_str(), "/" | "none"))
        || !at(args, path + 1).is_empty()
    {
        return None;
    }
    if !trusted {
        let value = &mut args[1];
        // AdGuard serializes numeric values after converting them to a number.
        if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
            *value = value.parse::<i16>().ok()?.to_string();
        } else if !matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "on" | "off" | "accept" | "reject"
        ) {
            return None;
        }
    }
    Some(())
}

fn href(args: &mut Vec<String>) -> Option<()> {
    if args.len() > 3 {
        return None;
    }
    let transform = match at(args, 2) {
        "" => None,
        "base64decode" if at(args, 1).starts_with('?') => Some("-base64"),
        _ => return None,
    };
    args.truncate(2);
    if let Some(transform) = transform {
        args[1].push(' ');
        args[1].push_str(transform);
    }
    Some(())
}

fn trusted_replacement(resource: &str, args: &mut Vec<String>) -> Option<()> {
    match resource {
        "trusted-replace-node-text" if args.len() == 4 => {
            let includes = args.remove(1);
            args.resize(3, String::new());
            if !includes.is_empty() {
                args.extend(["includes".to_owned(), includes]);
            }
        }
        "trusted-replace-argument" if (3..=4).contains(&args.len()) => {
            // Preserve shared primitive/JSON forms. AdGuard's quoted strings
            // and replace directives use a different value grammar.
            let raw = at(args, 2);
            if !matches!(
                raw,
                "undefined"
                    | "null"
                    | "true"
                    | "false"
                    | "emptyArr"
                    | "emptyObj"
                    | "noopFunc"
                    | "trueFunc"
                    | "falseFunc"
            ) && !raw.parse::<i16>().is_ok()
            {
                return None;
            }
            if args.len() == 4 {
                args.insert(3, "condition".to_owned());
            }
        }
        "trusted-prune-inbound-object" if (2..=4).contains(&args.len()) => {
            args.insert(1, "1".to_owned());
            if args.len() == 5 {
                args.insert(4, "stackToMatch".to_owned());
            }
        }
        "trusted-replace-outbound-text" if args.len() <= 3 && !at(args, 2).starts_with("json:") => {
        }
        _ => return None,
    }
    Some(())
}

fn passthrough(resource: &str, args: &[String]) -> Option<()> {
    let max = match resource {
        "disable-newtab-links" | "nowebrtc" => 0,
        "abort-on-property-read"
        | "abort-on-property-write"
        | "call-nothrow"
        | "close-window"
        | "noeval-if"
        | "prevent-canvas"
        | "prevent-requestAnimationFrame"
        | "remove-cookie" => 1,
        "abort-current-script"
        | "abort-on-stack-trace"
        | "m3u-prune"
        | "prevent-setInterval"
        | "prevent-setTimeout"
        | "prevent-innerHTML"
        | "remove-node-text"
        | "set-local-storage-item"
        | "set-session-storage-item"
        | "trusted-set-local-storage-item"
        | "trusted-set-session-storage-item" => 2,
        "adjust-setInterval"
        | "adjust-setTimeout"
        | "set-attr"
        | "spoof-css"
        | "xml-prune"
        | "trusted-set-attr"
        | "trusted-click-element"
        | "trusted-replace-fetch-response"
        | "trusted-replace-xhr-response" => 3,
        "trusted-suppress-native-method" => 4,
        // AdGuard can synthesize arbitrary response text through this name.
        // The bundled XHR helper has a different directive and response grammar.
        "trusted-prevent-xhr" => return None,
        _ => return None,
    };
    (args.len() <= max).then_some(())
}
