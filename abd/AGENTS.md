# AGENTS.md: daemon

`abd run` is the filtering daemon, started by `module/supervise.sh`. The
other subcommands are run by the module scripts and by the user from a root
shell.

## Two crates

What is done with a connection once it is here is `abd-engine`: `mitm`,
`clienthello`, `policy`, `filter`, `exclusions`, `tls`, `ca`, `relay`,
`limits`, `stats`, `update`, and with a lookup, `dns`; and so is what the user chose and what the
commands answer with: `apps`, `allow`, `pause`, `sources`, `rulecheck`,
`live`, `state`, `protocol`, and the commands themselves (`abd-engine/src/cli/`, `pins`),
which ask the program that runs them only whether the engine runs and to
have it read again (`cli::Host`). How the module brings the connections and lookups, connects
on as the app and has the apps trust the authority is this crate:
`capture`, `rules`, `dns`, `sockdiag`, `mounts`, `trust`, `watch`, `lifecycle`,
the UIDs of the chosen apps (`android/`), and the two commands that go on
(`update`, `watch` in `cli`). See `abd-engine/AGENTS.md` for what may not cross over. This
file is about both. Daemon setup, reconciliation, IPv6 probing and list
reloads live under `src/daemon/`. The program dispatches commands in `main.rs`.

## Code structure

- Dispatch from `main` and `cli` to command handlers and from `daemon` to
  platform adapters. Capture, trust and package discovery stay below orchestration.
- Keep package snapshots below group resolution and selection. Pass plain
  runtime snapshots to engine persistence.
- An unreadable selection must withdraw capture for unconfirmed UIDs.
  Surface the read failure and keep verified fail-open ordering.

## A connection

`daemon::setup::delivered` finds where a connection from TPROXY was going and for
whom, and whether it is one to carry at all (`capture::admitted`,
`Origin::of`); `mitm::handle` resets one that is not, or one too many
(`limits.rs`); `mitm::conduct` does the rest, and is what the tests drive.

- Anything accepted that the rules did not deliver for an app is reset.
  Any app can connect to the listener: never dial a destination taken from
  a connection that `admitted` did not pass. An `Origin` cannot be made
  any other way than by `Origin::of`.
- Every await on the app or the server is inside a timeout, and a
  connection handed on ends `LINGER_TIME` after its first half does.

- The ClientHello is peeked, never read, until the decision is made
  (`clienthello.rs`): a connection can be handed on byte for byte up to the
  moment the app is answered.
- The server is connected to and verified first. Whatever is wrong with it,
  the connection is handed on and the app sees the failure itself.
- Outbound sockets are the app's own to the kernel (`capture::connect_as`):
  they carry the app's fwmark and are created with the app's credentials
  (`capture::as_app`). Routing, VPNs, netd's per-app firewall and counters,
  and other modules' rules all go by these. Do not create one any other
  way, and keep nothing between the two halves of `as_app`.
- Bodies are streamed except for eligible responses matched by response
  rules. Up to 2 MiB of an original body can be held whole.
- `Hooks::dial` exists so that the tests can connect without root. Do not
  give `conduct` another way to reach the network.
- A handshake the client ends with an alert about our certificate pins the
  host at once; one that just ends counts towards `SILENT_LIMIT`.
- IPv6 is left alone only after connections found *no way* (`mitm::no_way`)
  and a probe of a server reached before fails too (`daemon/ipv6.rs`). Anything
  less lets an app turn filtering off for every app.

## A lookup

`dns/` brings the plain lookups of the active apps here and answers them.

- The routing rules (`dns/rules.rs`) are for exactly the UIDs TCP capture
  is for, set with them by `daemon::capture_rules`: none for apps whose
  connections are not captured. A failure there is said once and leaves
  connection filtering as it is; it is tried again at the next change of
  the apps, with no timer of its own, and given up after
  `dns::MAX_FAILURES` in a row.
- The routes go through a TUN device the daemon's process holds and
  never reads (`dns/anchor.rs`). Never make it persistent or let a child
  process inherit it: it is what ends lookup capture with the daemon.
- A lookup is served only if it came by our route (`serve::by_our_route`),
  from an app's UID (`Origin::of`), to an address that is not the
  device's. One the engine blocks for an active app is answered here with
  a lifetime of 0; any other is sent on unchanged as the app
  (`capture::udp_as`, `capture::connect_as`) and its answer relayed once it
  answers the query. Never answer with a block what was not judged.
- Every wait and table is bounded in `dns/serve.rs`. At a limit, the one
  lookup is dropped and the app's resolver asks again.
- What makes every lookup fail rather than one ends lookup capture
  (`Host::give_up`, `Unsent::Never`): the device goes, and lookups follow
  Android's routing. Never keep bringing lookups that cannot be answered.

## The rules that can cut an app off

`rules.rs` is the only thing here that can take an app's network away,
and `dns/rules.rs` its lookups.

- The lookup routing rules go in after their TPROXY chain and come out
  first. They need no xtables lock: `abd unhook` withdraws them before the
  TCP rules wait for one. They match only sockets with no source yet or
  the anchor's (`netlink::Source`): TPROXY takes only that source, and
  anything else they brought would be delivered to nothing.

- The chains of a family are written in one `iptables-restore`; the jumps
  go in after and come out before, the marking one last in and first out.
- Match the selected socket UIDs with standard owner rules. Only adjacent
  selected UIDs can share a range. Enable the named quota only after both
  families' rules and capture routes are ready.
- With no eligible, unpaused app in scope there are no rules. In All apps,
  resolve eligible personal UIDs before adding them to TCP and QUIC rules.
- `rules::unhook` must remove everything from any state, with no daemon
  running: `module/supervise.sh` depends on it after a crash. Disable the
  quota and withdraw both routing rules before waiting on xtables.
- The listening port is the kernel's choice at each start and is passed to
  `Rules::start`. There is no fixed port to squat.
- Its tests run the real logic against a model of the tools (`Fake`). A
  change to what is run needs the model and the tests changed with it, and
  `scripts/check-device.sh` run on an emulator.

## Trust

- With URL filtering on, stores are mounted in filtered personal app processes, by
  `abd trust mount` run as a process of its own (`trust.rs`): a process
  with more than one thread cannot enter another mount namespace. Never
  mount in the first namespace or in one that shares its mounts
  (`trust::spreads`): every process would see it.
- When: as the process starts (`watch::Starts`, `Mounts::started`). An app
  reads its roots once, at the first certificate it is shown, which may be
  one of a connection that is handed on: Chrome does, and with a mount
  made later than that it refuses us for every host. A process that was
  missed is given the mounts at its app's next connection, whatever
  becomes of that connection (`Mounts::ensure`), and the app is then told
  its roots changed and not decrypted for `SETTLE_TIME`.
- An app is answered with a certificate of ours only when `Hooks::trusted`
  says its processes have the mounts, and a refusal within `NEW_TIME` of a
  mount pins nothing. A refusal taken for a pin leaves a host unfiltered
  for 30 days: when in doubt, do not pin.
- After a change here, load a page in Chrome from a cold start on a
  device, with `ABD_VERBOSE=1`, and look for `reject` lines: there must be
  none. `scripts/check-device.sh` does not start Chrome.
- A copy of a store is built in place, a file at a time, is never emptied
  while it may be mounted, and is deleted only when no namespace has it
  (`trust::remove`). Mounts are read-only.
- `ca.rs`: two roots and a cross-certificate, one chain that ends in
  either. The test `one_chain_ends_in_either_root` is the contract with the
  two stores.

## What is never decrypted

- A host on an exclusion list (`exclusions.rs`), a server with an Extended
  Validation certificate (`tls::extended_validation`), a host an app pins
  (`policy.rs`), an app that could not be made to trust us.
- A new reason to decrypt less is cheap; a change that decrypts more needs
  a reason in the pull request.

## Filtering

- The full engine (`filter.rs`) judges decrypted requests. A separate
  engine holds the rules that block a host whatever the request, for
  connections that stay encrypted. Another engine matches response rules
  from trusted lists and the user's own rules. A matching response rule
  takes precedence over a request-level block.
- Preserve unknown page origin in both request matching and diagnostics.
- The lists come from the network: untrusted input. They select what is
  blocked, and what is hidden when page element hiding is on. Only trusted
  lists and the user's own rules can rewrite a text response.
  `abd-engine/src/update/` never replaces a list with what does not look like one.

## Hiding

`cosmetic.rs` puts styles and scripts into pages after any response rewrite.

- On by default and disabled by `cosmetic off` in the settings. It applies
  only to whole, uncompressed HTML pages requested as documents and only
  where the head of the page plainly begins
  (`cosmetic::place`). Anything else is passed on as it came: when in
  doubt, change nothing.
- A selector from a list goes into a page only through `cosmetic::style`.
  Whatever else is ever taken from a list for a page needs the same: a
  test that a hostile list cannot end the element or begin another.
- The policy of a page is changed only to permit the hash of the injected
  style or script. The script allowance is scoped to script elements.
  Preserve existing directives and only add an allowance where needed.
  Check the bounded head and all enforced policies before sending a script.
- What goes by the classes and ids of a page is hidden by a second style,
  at the page's end, of the names gathered as it passed (`cosmetic::Names`,
  `Filters::hiding_by_name`). The policy has been sent by then and is never
  changed for it: the style carries the page's own nonce or is left out
  (`cosmetic::inline`). The names are gathered without reading the page as
  a browser does; a name taken for one that is not hides nothing.
- The page is not held: `MAX_HELD` of its beginning at most, until the
  place is known. The names kept of it are bounded (`MAX_NAMES`), and so is
  a tag that comes in pieces.
- A change to what is put into pages, or to which pages, is said in
  SECURITY.md in the same commit.

## What is remembered

- `policy.rs` keeps pinned hosts by profile and package, never by UID, forgets
  them after 30 days, when the app is no longer filtered, and when a process
  of the app accepts its one try at its start (`policy/retry.rs`, in memory
  only: a try costs at most one connection per host per process start, and
  per `retry::INTERVAL` after a refused one). A try is no part of a
  decision: only the connection holding its ticket takes, ends or forgets
  it.
- Every table that grows with what apps do is bounded:
  `MAX_PER_PACKAGE`, `MAX_SILENT` (`policy.rs`), `retry::MAX_ENTRIES`,
  `MAX_LEAVES` (`ca.rs`), the connections (`limits.rs`), the lists
  (`filter::MAX_LIST`), the
  counts by app and hour (`stats::MAX_APPS`, `KEEP_HOURS`), the watchers
  and their queues (`live.rs`). A new one needs a bound too, and one app
  must not be able to use up what is another's.
- The apps and their UIDs are read again when `packages.list` is replaced
  (`watch.rs`).

## Watching

`abd watch` is the one way out of the daemon for what the apps talk to:
the host, and for a decrypted request its address, page and kind
(`live.rs`). It is told only to a process of root's that connected to
`watch.sock` in the data directory (mode 600, peer checked), and to
nothing else; none of it is written to disk or logged. While nobody
watches, a connection pays one atomic load and no event is made. Each
watcher has a bounded queue: one that falls behind loses events and is
told how many (`{"dropped":n}`), and one that stops reading is let go
after `WRITE_TIME`. A connection never waits for a watcher. Do not add
another reader of these events, or a file they go to.

## Files

- `main` sets the umask to 077 before anything else. The key is written by
  `ca::write_key` only: mode 600, never over an existing file.
- A process is signalled only through `lifecycle.rs`, which holds it by a
  pidfd and checks whose it is and what it runs. The pid files are the same
  wherever the module is installed, so a module stops the daemon of the one
  it replaces.
- The daemon of a module the app installed with root alone, known by the
  directory it runs from, ends as when asked to stop once the app has been
  out of Android's list of packages for `owner::CONFIRM`
  (`daemon/owner.rs`): nothing else could turn it off. A list that cannot be
  read or lacks what every device has ends nothing. The data stays.

## Power and performance

The daemon runs on every device all the time; its cost is the product's cost.

- No timers while nothing happens: counts are written some time after they
  change, the IPv6 retry timer exists only while IPv6 is left alone, the
  pause timer (`pause::Timer`) only while a pause is to end, and the
  owner's only while the app is missing from the list of packages.
- Two worker threads. Nothing is logged per connection.
- Per connection: one sock_diag question on a socket kept open, one peek,
  and the two handshakes. Do not add a file read, a process or a new
  socket to that path.
- Per lookup: one sock_diag and one route question on sockets kept open,
  and a socket each way.
- The TLS configurations are made once and shared (`tls.rs`): a session
  can be resumed only by the configuration that began it. Do not build one
  per connection.
- Sockets on the loopback side have Nagle's algorithm off: with it on, a
  handshake waits out a delayed acknowledgement, tens of milliseconds for
  every connection.
- A connection handed on is moved by the kernel (`relay.rs`), not copied.
- The lists are read from the engine's own format when they have not
  changed (`Filters::load_cached`).
- `cargo test --release -- --ignored --nocapture measures` prints what a
  connection, a megabyte and a start cost. Run it before and after a change
  meant to make something faster, and say both in the pull request.

## Tests

- `cargo test` runs both crates and needs no root and no device. `mitm/tests.rs`
  has a rig: a TLS
  server, a daemon in front of it and clients that trust the system root,
  the user root or only the server. Add a case there for any change to what
  happens to a connection.
- `sockdiag.rs` asks the real kernel about a connection of the test process.
