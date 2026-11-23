# AGENTS.md: engine

What is done with a connection of a filtered app, whoever brought it: the
host it asks for is read, and the connection is refused, handed on
untouched, or decrypted and its requests filtered. `abd` is the program
that uses it. The rules for the code here are in `abd/AGENTS.md`; this
file says what keeps the two apart.

## What the engine does not know

- How a connection came. It is given the connection and an `Origin`, and
  `Origin::of` is the only way to one: an app's, to somewhere else. What
  brings the connections checks for itself that each came its way
  (`capture::admitted` in `abd`).
- How to reach the network. `Hooks::dial` is the only way out for a
  connection carried; `update` connects to the addresses of the lists and
  nowhere else.
- How an app is made to trust the authority. `Hooks::trusted` says whether
  it does; an app that does not is not decrypted.
- Where on the device it is. It is given a directory (`paths.rs`) and the
  roots to verify servers against, and reads no other file.

Nothing here needs root, names a path outside its directory, or runs a
program. A change that would need any of these belongs in `abd`.

## Layout

Shared command dispatch lives in `src/cli/`, with handlers grouped by
command and text rendering in `render.rs`. The modules in `src/update/`
separate HTTP downloads and list validation from publication.
Connection handling lives in `src/mitm/`, with upstream transport, TLS
inspection, plaintext HTTP and request handling in separate modules.
`dns.rs` reads a lookup's question, judges it with the same host rules
and trusted sites, and builds the answer given to a blocked one; how a
lookup arrives and is sent on is each method's own. Root
and VPN share context setup and publication there. Each host retains its
own filter compilation schedule.

## Code structure

- Keep commands and protocol rendering above policy, parsing and storage.
  Put shared wire types in `protocol_types`; producers supply data to render.
- Keep request handling below the plaintext and decrypted HTTP drivers.
  Keep storage independent of the runtime that assembles its snapshots.
- Reuse `host` for basic hostname syntax and `clock` for epoch time.
  Keep caller-specific host policy in the caller.
- Use `files` for shared staging and replacement. Preserve the caller's
  existing durability and locking requirements.
- Keep list URL validation and naming in `list_url`, and bounded inventory
  and reads in `list_files`, below download, diagnostics and compilation.
- Treat unreadable configuration conservatively. Keep HTTPS encrypted until
  exclusions are ready and cache only complete filter generations.
- Fall back to rebuilding an unusable cache. An optional cache write may
  fail after a complete result is available.

## Tests

`cargo test` runs this crate with `abd`. The rig in `mitm/tests.rs` is the
contract: a new way of bringing connections must leave every case there as
it is.
