# AGENTS.md

ADBlocker filters the traffic of eligible apps on Android with browser
ad-blocking lists, including inside HTTPS. With root, a root module (a Rust
daemon and the module scripts) does the filtering;
without root, the same engine runs in the app's process behind a VPN
(`abd-vpn`). An Android app, whose APK is also the flashable module, is how
either is used. User documentation is in README.md, the design in
docs/how-it-works.md, what it means for the security of a device in
SECURITY.md, and building, checks and releasing in CONTRIBUTING.md.

## Layout

- `abd/`: the daemon (`abd run`) and its commands. See `abd/AGENTS.md`.
- `abd-engine/`: what the daemon does with a connection once it has it,
  as a library that needs no root. See `abd-engine/AGENTS.md`.
- `abd-vpn/`: the engine as the native library `libabd_vpn.so`, for the app
  without root: a TCP stack in user space over the VPN's descriptor. Its
  interface with the app is `docs/engine-jni.md`. Packet ingress and socket
  flows live under `abd-vpn/src/tunnel/`.
- `module/`: root module scripts. See `module/AGENTS.md`.
- `android/`: the app (Kotlin, Compose, Material 3 Expressive), and in the
  same APK the LSPosed module (`android/hook`). See `android/AGENTS.md`.
- `testdata/protocol/`: what each `abd ... --json` command prints, read by
  the tests of the daemon and of the app.
- `scripts/`: `build.sh` (full build), `install-on-emulator.sh` (the module
  on an emulator without a root manager), `check-device.sh` (on-device
  check that an app's traffic is filtered, and that the app keeps its
  network when the daemon is killed) and `check-module-*.sh` (the module
  scripts against a model of the device, on the host).

## Commands

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check              # advisories, licenses and sources (deny.toml)
for s in module/*.sh scripts/*.sh; do bash -n "$s"; done
./scripts/build.sh --locked  # builds the native components and stages them for the app
(cd android && ./gradlew spotlessCheck test)
./scripts/install-on-emulator.sh && ./scripts/check-device.sh   # with an emulator running
```

Run the last line after a change to `abd/src/rules.rs`, `abd/src/trust.rs`,
`abd/src/capture.rs`, `abd/src/dns/` or `module/`. Host tests cover rule logic; a device is needed to check Android and kernel behavior.

## Invariants

- Fail open. Disable TCP marking through the named quota,
  then withdraw the routes and the lookup routing rules before waiting for
  the packet filter lock on daemon exit. Lookup routes go through a device
  the daemon's process holds, so they end with it, however it ends.
  Lookup capture that cannot be set up leaves connection filtering running.
  Pass connections through when the daemon cannot verify the server or the
  app rejects the local CA, unless a rule blocks their host. Android VPN lockdown controls access after the
  VPN stops.
- Root refuses UDP 443 for active app UIDs through the capture quota.
  The VPN refuses recognized client QUIC Initials for active apps so they
  can retry over TCP.
- URL filtering is global and on by default in both methods
  (`Settings::UNSAID`). When on, all filtered apps can use request-level
  rules where available. The user can turn it off.
- Verify upstream TLS against Android system roots before signing a leaf
  certificate. Keep HTTPS encrypted until the exclusion policy is ready.
  See SECURITY.md for the limits of local TLS inspection.
- When in doubt, block less. A rule is applied only where what it needs to
  know is known (`abd-engine/src/filter.rs`). A request that names no page
  is judged as AdGuard judges one without a referrer: it is first-party
  and of none of the pages a rule names.
  A host that a rule blocks whole is refused before its requests are read
  unless an exception names it. An exception that names no host, or a
  host that no rule blocks whole, is not one to such a block.
- Choosing a named shared UID in Root mode chooses its current and future
  members in that Android profile. In All apps, a personal group is included
  or excluded together. They share one selection, pause and record
  of activity. Persistent choices identify the group by Android's shared
  user name and profile, independent of its numeric UID.
- The default scope filters chosen apps. All apps includes confirmed personal
  app UIDs (10000–19999), except ADBlocker and user exclusions. Work apps
  still require selection. Out-of-scope, excluded and system traffic is
  never read, and these apps see no module mounts, certificates or changed
  files. Without root, the VPN and, once the user installs it, the certificate in the user
  store are visible to every app; SECURITY.md says so. In a managed work
  profile, any public CA provisioned by its administrator follows the
  profile's normal trust rules. Never mount stores into work apps.
- The LSPosed module lets an app's own certificate checks pass only a
  chain that Android's default validation for the app accepts and whose
  trust anchor is the local root in the module's copy of the store, as
  that process's mount table shows it mounted, compared by subject and
  key. Every other result, and any result where the hook
  fails, stays the app's. A failure on the server's name always stands:
  the leaf must name the host the check knows, and the socket and engine
  forms are checked again through the same form of Android's manager.
- Only what the rules delivered is carried, and only as the app it came
  from: never a connection made to the listener directly, and never one on
  behalf of nobody or of the system (`capture::admitted`, `Origin::of`).
- Nothing an app, a server or a list sends can make the daemon wait, grow
  or hold without a bound. A new wait gets a timeout and a new table a
  limit, in the same change.
- A daemon that cannot go on ends: release builds abort on panic, and the
  supervisor removes the rules. Do not catch a panic to keep it running.
- The module must not be able to break booting: no `post-fs-data` script, no
  `system/` overlay, no SELinux rules.

## Contracts between components

- The fwmark bits `MARK_CAPTURE` and `MARK_BYPASS` (`abd/src/capture.rs`)
  are outside the bits netd uses and the bit other root modules mark with.
  `scripts/check-device.sh` checks both bits and reads the kernel names from
  `abd/src/rules.rs`, which owns the chain names, quota and routing table,
  and from `abd/src/dns/mod.rs`, which owns the lookup table, rule priority,
  chain and device.
- `module/*.sh` use the commands `abd run`, `trust remove`, `update`,
  `unhook`, `module-stop`, `app list` and `stats`, and the files
  `abd.pid`, `supervisor.pid`, `abd.log`, `boot-guard` and `boot_pending` in
  `/data/adb/adblocker`. Change
  a command or a file name together with the scripts.
- The module is in `/data/adb/modules/adblocker` when a root manager has
  it, and in `/data/adb/adblocker_direct`, with a launcher in
  `/data/adb/service.d/adblocker.sh`, when the app installed it
  (`module/direct.sh`). The manager's is in use once it has `bin/abd`.
  `module/*.sh`, `ModulePaths` in the app, `abd/src/daemon/owner.rs` and
  the device checks all go by this; change them together.
- The app reads the daemon only through `abd ... --json`, whose outputs are
  in `testdata/protocol/`. Change an output, its case, `abd-engine/src/protocol.rs`
  and `android/core/data/.../AbdProtocol.kt` in the same commit. An invalid
  command or argument exits with 2.
- The app uses the root-only `abd handoff begin|rollback|finish <uuid>`
  commands when switching from VPN. They print no output on success.
  `begin` keeps a checkpoint for the same canonical lowercase UUID, and
  takes a fresh checkpoint when a new UUID supersedes a stale one.
  `rollback` restores the saved settings, prunes inactive list copies,
  requests a reload and removes the checkpoint. `finish` removes it after
  publication. Both succeed when it is absent, and fail when a different
  UUID owns it.
- `android/hook` finds the local roots in the stores by the names
  `trust::store_name` gives them (`LocalAuthority.kt`), and recognizes
  the copies the daemon mounts by their directories, `store` and
  `user-store` of `/data/adb/adblocker` (`ModuleMounts.kt`). Tests in
  `abd/src/trust.rs` keep both; change them together.
- What a released version stored in `/data/adb/adblocker` is read by every
  later version.
- `module/module.prop` is the single source of the version. A release tag
  must equal `version`.

## Conventions

- Code, comments, commit messages and documentation in English. Comments
  explain why, not what.
- Commit subjects are plain imperative sentences without prefixes
  ("Remove the rules before the daemon stops"); the body says why. One topic
  per commit and per pull request. Do not add AI attribution, co-author
  trailers or session links to commits or pull requests.
- Use GPL-3.0-only headers for project code. Preserve upstream copyright and license notices in adapted
  code. Keep the notices listed in THIRD_PARTY_NOTICES.md.
- `unsafe` is denied in the workspace. A module that needs it says so at
  its top (`#![allow(unsafe_code, reason = "...")]`), and every block
  carries a `// SAFETY:` comment saying why it is sound; clippy rejects one
  without.
- Rust tests are snake_case statements of the behaviour
  (`nothing_is_signed_for_a_server_that_cannot_be_verified`), with no
  `test_` prefix. Add or update tests for the code you change.
- Dependencies stay on their latest stable versions. Dependabot proposes
  updates for Cargo and GitHub Actions.

## Code structure

- Limit production source files to 800 lines, or 500 for files with Compose
  functions. Keep functions within 80 lines and split by responsibility.
- Event loops and command entry points dispatch to named handlers.
- Give each concept one name and one implementation. Reuse shared helpers
  and keep module dependencies acyclic.
- Handle, return or log errors. If a failure is safe to ignore, explain why
  next to the operation. Keep repeated logs bounded and follow the privacy rules.
- Distinguish complete operations from best-effort work. Report partial
  results explicitly when callers rely on completion.
- Preserve behaviour, command output, schemas and screenshots in refactors.
  Pair behaviour changes with focused regression tests.

### VPN engine

- Keep JNI and session lifecycle above engine assembly. Dispatch tunnel
  events to packet and socket handlers below it.
- Reuse `abd-vpn`'s `platform` for owner lookup and protected sockets and
  `datagram` for common UDP support. Keep both below engine assembly.
- Keep datagram drops and queues bounded. If VPN selection is unreadable,
  stop filtering. Refuse connections without an attributable app.

## Privacy rules

- ADBlocker sends no telemetry. The filtering engine forwards allowed
  traffic to its destination and downloads subscribed lists.
- Never log what the apps talk to: hosts, addresses, or an app with either.
  The log says what ran and how it ended; `stats` holds counts only.
  `ABD_VERBOSE` lifts this for development and is never on by default.
- The `pinned` file names hosts with the app that pins them. It stays in
  `/data/adb/adblocker`, readable by root only, and is never exported.
- So do the other files there that name an app or a host: `apps` (chosen apps,
  profile serials, and personal exclusions for All apps), `pauses` (the apps
  the user paused), `allow` (the hosts the user trusts) and `stats` (counts by
  package and hour). `forget` and its staging file `forget.new` name a
  package and maybe a host from `abd pinned forget` until the running
  engine confirms persistence.
  `watch.sock` is root's only, and what happens is said on it only while somebody
  listens. `state` can name profile serials whose HTTPS trust is ready. It
  stays private. `settings` holds the filtering scope and names no apps or
  hosts. `list-info/`, `lock` and `boot-guard` name neither an app nor a host,
  nor does `boot_pending`, which counts the boots in a row that started the
  module and did not finish.
  `handoff` and its staging file `handoff.new` can contain the
  previous app choices and exclusions, settings, list addresses, user rules
  and trusted hosts. They stay readable by root only and are never exported. A new file there is
  added to this list, and says here what it names.
- HTTP validators in `list-info/` and `exclusions/info/` can name a list
  publisher and its resource. They stay in the private data directory.
- The startup cache in `cache/` contains compiled lists. Its
  `no-element-hiding` index can contain host names and URL patterns from
  page exceptions. It stays in the private data directory.
- The on-demand cache in `cache/diagnostic-v1` contains compiled rules
  and an index of their source lists. Like the lists it can name hosts. It
  stays in the private data directory and never records what was checked.
- The authority's key stays in `/data/adb/adblocker/ca`, readable by root
  only, and is destroyed on uninstall. Without root it stays in the app's
  private directory, and is destroyed when the app is uninstalled.
- The app declares the network permission for the VPN method only: the
  engine downloads the lists and connects onward for the filtered apps from
  the app's process. With the root method the module does both, as root,
  and the app itself does not connect. One APK carries both methods, so
  the permission is there with either.
- The app collects hosts and URLs from `abd watch` only during a recording
  the user explicitly started. A recording can continue in the background.
  Its bounded results stay in memory and are never written or logged.

## Documentation

Describe current behavior in README.md, SECURITY.md and docs/. Keep
security limits in SECURITY.md and use release notes for changes.

## Pull requests

Branch from `main`. Run the checks in CONTRIBUTING.md. For changes to
Android or kernel behavior, include the device checks you ran.
