# Protocol cases

The app reads the daemon's state only through `abd` commands run as root,
each printing JSON. Every file here is one command's output for a known
state, and the tests of both sides read it:

- The daemon tests in `abd-engine/src/protocol.rs` and `abd/src/apps.rs`
  build each output from the state the file describes and compare it as JSON.
- `AbdProtocolTest.kt` (`:core:data`) parses each file and checks the
  meaning the app takes from it.

A change to an output changes its file, and both sides with it, in the same
commit.

| File | Command | Notes |
| --- | --- | --- |
| `overview.json` | `abd overview --json <start1> ... <start8>` | Status, seven daily counts and subscriptions from one refresh |
| `status.json` | `abd status --json` | Running daemon, globally not paused |
| `status-filtering-apps.json` | `abd status --json` | The root daemon distinguishes the apps actively being filtered from the saved selection |
| `status-stopped.json` | `abd status --json` | No daemon; what is known from the files |
| `apps.json` | `abd app list --json` | One app filtered by host only, one paused |
| `apps-work.json` | `abd app list --json` | Personal and work installations of the same app. Only the work installation is paused and lacks HTTPS trust |
| `apps-unavailable.json` | `abd app list --json` | Saved shared-UID and unresolved app selections, with their modes and effective filtering off |
| `app-eligibility.json` | `abd app eligibility --json` | Root only. Personal package eligibility and named shared UID groups |
| `app-profiles.json` | `abd app profiles --json` | Root only. A managed profile of the personal user and its eligible package names, including a work-only app |
| `app-profiles-partial.json` | `abd app profiles --json` | One known profile could not be queried, another has no eligible apps, and a healthy sibling still has its package list |
| `stats.json` | `abd stats --json <start1> ... <start8>` | Seven daily pairs of blocked requests and hosts, across retained apps and per app |
| `lists.json` | `abd lists --json` | One list disabled, one that failed |
| `update.jsonl` | `abd update --json` | One event a line |
| `pinned.json` | `abd pinned --json` | |
| `allow.json` | `abd allow list --json` | |
| `rules-check.json` | `abd rules check --json` | For `rules.txt` checked with user-rule permissions |
| `rules.txt` | input of `abd rules check` and `abd rules set` | |
| `check.json` | `abd check --json [--app <app>] <url> <page> <kind>` | |
| `watch.jsonl` | `abd watch --json` | One event a line |

## Conventions

- Times are seconds since the epoch, except `time` in `watch.jsonl`, which
  is milliseconds.
- A field that has no value is `null`, except `profile_serial` is absent for
  a personal app. A reader ignores fields it does not know.
- `app list` carries the package name or named group key in `package` and
  Android's stable user serial in `profile_serial` for a work selection.
  Work keys in commands, pins, activity and stats include the `serial:` prefix.
- In `app profiles`, `packages` is `null` when the profile's package query
  failed or the profile appeared while the other profiles were queried.
  `[]` means the query succeeded with no eligible packages. An unreadable
  shared package database or an unavailable profile roster fails the command.
- Root `app configure --keep-work` takes app identities on standard input.
  It replaces personal selections, keeps all existing work selections and
  their modes, and validates and adds newly chosen work apps.
- A command the daemon does not know exits with status 2: the app takes
  that as "the module needs an update".
- Errors go to standard error, with a non-zero exit status, and are never
  shown in the app without a message of its own.

## Values

- `days` (`stats.json`): seven `[requests, hosts]` pairs across retained
  apps, oldest first, with zeroes on dates without blocks. The eight
  command arguments are the Unix seconds of consecutive local day starts,
  ending with the date after today. A UTC hour is assigned by its start.
- `apps` (`stats.json`): the same seven pairs, keyed by app identity, for
  each app with an hourly record in the requested window.
- `apps` (`status.json`): number of saved app selections. Root also reports
  `filtering_apps` from current daemon state, with zero when filtering is idle.
- Root inventories list named shared UIDs in `groups`, each with the raw
  Android shared user name in `id` and the installed package names in
  `packages`. In work inventories, both arrays are null if the profile
  could not be queried. The saved group key is `@name` for the personal
  user and `serial:@name` for a work profile. Outputs with a separate
  `profile_serial` put `@name` in `package`. Activity, pauses and
  statistics all use that same selection identity.
- In `app eligibility`, `shared_uid` identifies a package with a shared
  network identity. A named group can be selected while Android can verify
  its identity. An absent package or an `unavailable` result means that
  its ownership could not be established.
- `scope` (`status.json`) is `chosen` by default or `all` for eligible
  personal app UIDs. `app selections --json` (`app-selections.json`)
  returns the scope, explicit choices, and personal exclusions. In All apps,
  `app list --json` returns effective personal members plus chosen work apps.
- `effective.http` and `effective.https` (`apps.json`) each carry a
  `level` (`hosts` or `full`) and a nullable `reason`. Both report
  `url_filtering_off` when URL filtering is off. With it on, HTTPS can
  report `ca_not_installed`, `exclusions_pending` or
  `profile_trust_unavailable`. An unavailable saved app in Root mode
  reports `off` and `unavailable_reason` (`shared_uid` or `unavailable`).
- `boot_guard` (`status.json`): when the module turned itself off because
  two boots in a row did not finish, or `null`. It stays turned off, in the root
  manager too, until it is started again; starting it clears this.
- `cosmetic` (`status.json`): whether elements the lists name are hidden
  on decrypted pages. On by default (`abd cosmetic on|off`).
- `url_filtering` (`status.json`): the global option, on by default
  (`abd url-filtering on|off`).
- `verdict` (`watch.jsonl`): `blocked` (a filtered HTTP request, cleartext or
  decrypted), `allowed` (likewise, let through), `host_blocked` (the
  connection refused for its host), `passed` (handed on, unread).
- `reason`, for `passed` only: `pinned` (the app does not accept us for the
  host), `hosts_only` (URL filtering or HTTPS inspection is unavailable), `excluded` (an exclusion
  list), `trusted` (the user's allow list), `sensitive` (the server's
  certificate says not to decrypt), `no_name` (no host name in the
  ClientHello), `untrusted` (the app cannot be made to trust us yet),
  `unverified` (the server could not be verified).
- `kind`: what `mitm::kind` returns, such as `document`, `script`, `image`,
  `stylesheet`, `font`, `media`, `xmlhttprequest`, `websocket`, `other`.
- `state` (`update.jsonl`): `fetching`, `updated`, `unchanged`, `failed`.
- `kind` (`rules-check.json`): `invalid` identifies malformed or unknown
  input. `unsupported` identifies recognized syntax the engine cannot apply.
  `inapplicable` identifies a rule that needs unavailable request context.
- `using_previous_address` (`lists.json`): true when the active copy came
  from the list's former address and is kept until the new address supplies
  a usable copy. The field is omitted otherwise.
- `source` (`check.json`): the address of the list the rule is in, `rules`
  for the user's own rules, or `null`.
- A `{"dropped":n}` line in `watch.jsonl` says that `n` events were left
  out because the reader fell behind.


## Rule checks

`rules check --json` reports the number of active rule lines as `rules`.
Each issue contains its 1-based `line`, trimmed `text`, `kind` and `reason`.
Includes and malformed preprocessor directives are issues outside the rule
count. The command and fixture use user-rule permissions. The list checker
can also report `untrusted` for rules that need an approved source.

| Kind | Reason | Meaning |
| --- | --- | --- |
| `inapplicable` | `option:<name>` | The engine has no context for the named modifier. |
| `invalid` | `address-endpoint` | The address is not a supported IP literal and optional port. |
| `invalid` | `address-expression` | The regular expression for the address cannot be compiled. |
| `invalid` | `address-modifiers` | The address rule has invalid or incompatible modifiers. |
| `invalid` | `app-condition` | The app condition has missing, repeated, or incompatible options. |
| `invalid` | `app-pattern` | The app selector is invalid or exceeds the pattern budget. |
| `invalid` | `cookie-attributes-on-exception` | A cookie exception specifies attributes. |
| `invalid` | `cookie-duplicate-attribute` | A cookie attribute appears more than once. |
| `invalid` | `cookie-invalid-attribute` | The cookie attribute is unknown or malformed. |
| `invalid` | `cookie-invalid-expression` | The cookie expression cannot be compiled. |
| `invalid` | `cookie-invalid-max-age` | The cookie expiry value is invalid. |
| `invalid` | `cookie-invalid-name` | The cookie name is invalid. |
| `invalid` | `cookie-invalid-same-site` | The SameSite setting is invalid. |
| `invalid` | `cookie-unescaped-separator` | The cookie expression contains an unescaped separator. |
| `invalid` | `cosmetic-scope` | The page rule uses a prohibited generic or negated scope. |
| `invalid` | `cosmetic-syntax` | The page rule has invalid syntax, a selector, or a declaration. |
| `invalid` | `cosmetic-unsafe` | The cosmetic text cannot be safely placed in the page. |
| `invalid` | `csp-empty` | The CSP modifier has no policy value. |
| `invalid` | `csp-invalid-value` | The policy cannot be used as an HTTP header value. |
| `invalid` | `csp-method` | The CSP modifier cannot be combined with a method constraint. |
| `invalid` | `csp-negated` | The CSP modifier is negated. |
| `invalid` | `denyallow-duplicate` | The denyallow modifier appears more than once. |
| `invalid` | `denyallow-empty-domain` | The exclusions include an empty entry. |
| `invalid` | `denyallow-invalid-domain` | An excluded domain is not a valid hostname. |
| `invalid` | `denyallow-matcher` | The destination exclusions cannot be represented by the matcher. |
| `invalid` | `denyallow-negated-domain` | The exclusions contain a negated domain. |
| `invalid` | `denyallow-negated-modifier` | The denyallow modifier is negated. |
| `invalid` | `denyallow-pattern-names-host` | The pattern already constrains the destination host. |
| `invalid` | `denyallow-regex-domain` | An excluded domain is a regular expression. |
| `invalid` | `denyallow-wildcard-tld` | An excluded domain uses a wildcard top-level domain. |
| `invalid` | `denyallow-with-response-action` | The rule combines denyallow with an incompatible action. |
| `invalid` | `denyallow-with-to` | The rule also specifies a to modifier. |
| `invalid` | `hls-empty-pattern` | The playlist rule has an empty segment pattern. |
| `invalid` | `hls-invalid-flags` | The segment expression has unrecognized flags. |
| `invalid` | `hls-missing-delimiter` | The segment expression has no closing delimiter. |
| `invalid` | `hls-unescaped-separator` | The segment pattern contains an unescaped separator. |
| `unsupported` | `html-generation-limit` | The HTML rule exceeds the generation's count or compiled expression budget. |
| `invalid` | `html-attribute` | The selector contains a malformed attribute condition. |
| `invalid` | `html-domain` | The HTML rule has an invalid domain constraint. |
| `invalid` | `html-length` | The content length constraint is malformed. |
| `invalid` | `html-regex` | The content expression cannot be compiled. |
| `invalid` | `html-selector` | The HTML rule does not contain a usable selector. |
| `invalid` | `javascript-empty` | The JavaScript body is empty. |
| `invalid` | `javascript-no-domains` | A raw JavaScript rule requires a domain. |
| `invalid` | `javascript-reserved-name` | The rule uses an internal scriptlet name. |
| `invalid` | `javascript-unsafe` | The JavaScript text cannot be safely placed in the page. |
| `invalid` | `jsonprune-value` | The JSON path or its expression is invalid. |
| `invalid` | `malformed-directive` | The preprocessor found a malformed or unmatched conditional directive. |
| `invalid` | `network-domains` | The rule has no usable matcher domain. |
| `invalid` | `network-syntax` | The network rule cannot be parsed. |
| `invalid` | `option:<name>` | The matcher cannot read the named option or its value. |
| `invalid` | `removeheader-invalid-name` | The header name is not an HTTP token. |
| `invalid` | `removeheader-protected-header` | The rule names a protected header. |
| `invalid` | `removeparam-negated-modifier` | The removeparam modifier itself is negated. |
| `invalid` | `replace-value` | The replacement or its expression is invalid. |
| `invalid` | `response-condition` | The action has an invalid request condition. |
| `invalid` | `response-qualifier:<name>` | The action cannot be combined with the named option. |
| `invalid` | `scriptlet-syntax` | The AdGuard scriptlet call is malformed. |
| `invalid` | `style-syntax` | The AdGuard style rule is malformed. |
| `invalid` | `unsafe-scriptlet` | The scriptlet text cannot be safely placed in the page. |
| `invalid` | `urltransform-value` | The URL transformation or its expression is invalid. |
| `invalid` | `xmlprune-value` | The XML path or its expression is invalid. |
| `unsupported` | `address-limit` | The address rule table is full. |
| `unsupported` | `adguard-cosmetic-matcher` | The AdGuard page rule cannot be represented by the cosmetic matcher. |
| `unsupported` | `cookie-too-long` | The cookie rule or expression exceeds its size limit. |
| `unsupported` | `cosmetic-unsupported` | The page rule uses syntax unavailable to the cosmetic matcher. |
| `unsupported` | `csp-reporting` | The engine does not add CSP reporting directives. |
| `unsupported` | `hls-inner-modifiers` | The segment pattern contains nested modifiers. |
| `unsupported` | `hls-unsupported-pattern` | The segment pattern cannot be compiled by the playlist matcher. |
| `unsupported` | `html-attribute-operator` | The attribute operator is unavailable to HTML filtering. |
| `unsupported` | `html-child-content` | The engine cannot combine this child selector and content condition. |
| `unsupported` | `html-content-attribute` | This form of the content attribute is unavailable. |
| `unsupported` | `html-lookaround` | Lookaround is unavailable in this content expression. |
| `unsupported` | `html-pseudo-class` | The pseudo-class is unavailable to HTML filtering. |
| `unsupported` | `html-regex-flags` | The content expression uses unsupported flags. |
| `unsupported` | `html-response-header` | HTML filtering does not match response headers. |
| `unsupported` | `html-size` | The HTML rule exceeds a size or complexity limit. |
| `unsupported` | `include-not-followed` | The preprocessor does not load the referenced list. |
| `unsupported` | `javascript-count` | The JavaScript table has reached its entry limit. |
| `unsupported` | `javascript-length` | The JavaScript body exceeds its size limit. |
| `unsupported` | `javascript-table-size` | The JavaScript table has reached its byte limit. |
| `unsupported` | `javascript-template` | Raw JavaScript cannot contain scriptlet template markers. |
| `unsupported` | `network-regex` | The network expression uses unsupported syntax or exceeds the matcher limit. |
| `unsupported` | `option:<name>` | There is no implementation of the named modifier. |
| `unsupported` | `regex-source-domain` | The matcher cannot read a regular expression as a source domain. |
| `unsupported` | `removeheader-required-for-request` | The engine needs this header to forward the request. |
| `unsupported` | `removeheader-required-for-response` | The engine needs this header to forward the response. |
| `unsupported` | `removeparam-expression` | Removal by regular expression is unavailable. |
| `unsupported` | `removeparam-inverted` | Removal by an inverted selector is unavailable. |
| `unsupported` | `removeparam-remove-all` | Removing all parameters is unavailable. |
| `unsupported` | `removeparam-unsupported-name` | The name is outside the supported literal parameter syntax. |
| `unsupported` | `response-limit` | The response rule is longer than the supported limit. |
| `unsupported` | `scriptlet-arguments:<name>` | The scriptlet arguments cannot be translated. |
| `unsupported` | `scriptlet-no-counterpart:<name>` | No bundled scriptlet can run the named call. |
| `unsupported` | `style-declarations` | The style declarations cannot be injected safely. |
| `untrusted` | `javascript-untrusted` | Raw JavaScript requires a trusted source. |
| `untrusted` | `response-trust:<name>` | The named action requires an approved source. |
| `untrusted` or `unsupported` for user rules | `scriptlet-trust:<name>` | The named scriptlet requires an approved subscription. |
