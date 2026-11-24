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
| `rules-check.json` | `abd rules check --json` | For `rules.txt` on standard input |
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
  on decrypted pages (`abd cosmetic on|off`). Off unless the user turned
  it on: it changes the pages.
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
- `kind` (`rules-check.json`): `cosmetic` (element hiding or scriptlets:
  hiding is applied only while `cosmetic` is on, scriptlets never; the
  check names them either way) or `invalid` (not a rule).
- `source` (`check.json`): the address of the list the rule is in, `rules`
  for the user's own rules, or `null`.
- A `{"dropped":n}` line in `watch.jsonl` says that `n` events were left
  out because the reader fell behind.
