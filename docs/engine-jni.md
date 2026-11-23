# Engine interface without root

Without root, the app filters through a VPN: a `VpnService` routes the
chosen apps into a TUN interface, and the engine, built as the native
library `libabd_vpn.so` (crate `abd-vpn`), turns its packets into
connections and filters them as the root daemon does. This document is the
contract between that library and the app. A change to it changes both
sides in the same commit.

## Library

- Name: `abd_vpn`, loaded with `System.loadLibrary("abd_vpn")`. Built for
  arm64-v8a and x86_64.
- One engine per process. A panic in native code aborts the process, which
  takes the VPN down with it. Android applies its VPN lockdown setting.

## Functions

Declared in `li.mercury.adblocker.core.engine.Engine`:

| Function | Purpose |
| --- | --- |
| `open(dataDir: String, roots: Array<ByteArray>, callbacks: Callbacks): Long` | Opens the engine on `dataDir` (the app's `filesDir/engine`). `roots` are the DER certificates servers are verified against. Returns a handle, or 0 if the engine cannot open. |
| `command(handle: Long, args: Array<String>, stdin: ByteArray?): Result` | Runs one command of the `abd` dispatcher, synchronously, on the caller's thread. |
| `stream(handle: Long, args: Array<String>, sink: LineSink): Long` | Runs `watch --json` or `update --json [--list <url>]`. Returns a stream id. |
| `cancel(handle: Long, id: Long)` | Ends a stream, as closing standard input ends it under root. |
| `start(handle: Long, tunFd: Int, ipv6: Boolean, dnsServers: Array<String>): Int` | Takes the TUN file descriptor and starts the stack. Returns 0, or an exit code. |
| `stop(handle: Long)` | Stops the stack and closes the descriptor. |
| `close(handle: Long)` | Releases the engine at process end. |

- `open` without `start` serves commands while the VPN is off: lists, rules,
  stats and settings are available whether or not filtering runs. When
  `open` returns 0, the reason is in logcat under the tag `abd`.
- `command` runs every command except `watch` and `update`, which only
  `stream` runs. Commands that exist only under root (`run`, `trust`,
  `unhook`, `module-stop`) exit with 2. `command` may be called from several
  threads at once; the engine serialises changes to its files.
- `start` takes ownership of the descriptor: the app passes
  `ParcelFileDescriptor.detachFd()`. The engine makes it non-blocking and
  closes it on `stop`. `start` after `stop`, with a new descriptor, starts
  the stack again. `stop` returns once the stack has stopped and the
  descriptor is closed. Calling `close` on a started engine stops it first.
  `dnsServers` lists numeric port-53 endpoints from the default network's
  `LinkProperties` in resolver order. The engine reads up to four endpoints. Formats include
  `192.0.2.53:53` and `[fe80::53%4]:53` (numeric IPv6 scope ID).
- `Result(exit: Int, stdout: ByteArray, stderr: ByteArray)`. Exit status and
  output are those of `abd`: 0 on success, 2 for a malformed command, 1 for
  any other error, and the JSON in `testdata/protocol/`.
- `stats --json` takes eight nondecreasing Unix timestamps in seconds,
  from the start of the first local date to the start of the date after
  today. `days` and each entry of `apps` hold seven `[requests, hosts]`
  pairs, oldest first. A UTC hour belongs to the date containing its
  start. Days without counts have `[0, 0]`, and `apps` includes packages
  with at least one recorded hour in the window.

- `overview --json` takes the same eight timestamps. Its `status`, `stats`
  and `lists` fields have the same shapes as the corresponding commands.
  The status update time comes from the subscriptions in this response.

## Roots

`roots` are the system store as the app sees it: the `AndroidCAStore` key
store entries whose alias starts with `system:`. The key store leaves out
roots the user turned off, which a directory listing would not. Entries
starting with `user:` are never passed: ADBlocker's own authority is among
them.

## Callbacks

Called from engine threads.

| Callback | When | Purpose |
| --- | --- | --- |
| `protect(fd: Int): Boolean` | Once per upstream socket | `VpnService.protect`. |
| `ownerUid(protocol: Int, local: ByteArray, localPort: Int, remote: ByteArray, remotePort: Int): Int` | TCP connections, UDP 443, recognized QUIC Initials on other ports, and virtual DNS queries | `ConnectivityManager.getConnectionOwnerUid`; -1 if unknown. |
| `uidOf(packageName: String): Int` | At start and at each reload | The package must be the sole owner of the UID. Returns -1 for a missing or shared UID. |
| `personalAppIdentities(exclusions: String): String` | All apps inventory and reload | Takes newline-separated package or group exclusions. Returns one `UID identity` per line for attributable personal app UIDs outside those exclusions and the VPN app's own UID. |
| `userCaInstalled(): Boolean` | At start and at each reload | Whether the authority is in the user store, by fingerprint. |
| `stopped(code: Int)` | Once, if the stack ends by itself | The app stops the `VpnService`. |

The app runs the `reload` command when packages are installed or removed,
and when the HTTPS setup has installed or removed the certificate.

`LineSink` has `line(text: String)` and `end(exit: Int)`. For a started
stream these run in order on a thread separate from packet processing.
`end` is called once, also after `cancel`, and no `line` follows it.
A watch that falls behind loses events and is told how many. A watch
whose app channel stays full for 30 seconds ends with an error. Closing
the channel releases any callback waiting for the reader and cancels the
native stream. List updates deliver progress and the final summary in
order. With `--list <url>`, the update fetches only that enabled subscription.

In `ownerUid`, `protocol` is the IP protocol number (6 for TCP, 17 for UDP),
`local` and `remote` are 4 or 16 bytes in network order, and `local` is the
app's end of the connection.

## Decryption

HTTPS URL filtering requires the global `url-filtering` setting, on by
default, and a true result from `userCaInstalled()` at the last start or
reload. The bundled HTTPS exclusions must also be ready. The engine
applies domain rules when HTTPS remains encrypted. `app list --json`
reports the effective HTTPS level and reason in `effective.https`.

`ca export` writes the authority's certificate to standard output: PEM by
default, DER with `--der`. The HTTPS setup saves it for the system's
certificate installer.

## VPN

What `VpnService.Builder` sets:

- MTU 1500.
- One IPv4 address, with the route `0.0.0.0/0`.
- One IPv6 address, with the route `::/0`, only while the network beneath
  reaches the internet over IPv6: it has a global address and a default
  route. Without IPv6 in the VPN, the system answers the chosen apps' IPv6
  connections as unreachable at once, and they use IPv4. The engine would
  refuse them too, each after trying to connect onward; the system does so
  without the try.
- `addAllowedApplication` for each chosen app, and for nothing else.
- With underlying resolvers, `addDnsServer("10.120.38.2")` on the
  `10.120.38.0/30` subnet. With IPv6, also
  `addDnsServer("fd8b:5f3a:9c21::2")` on `fd8b:5f3a:9c21::/126`.
  Without resolver addresses, Android keeps its inherited DNS setup.
- `setMetered(false)`.

Changes to the selected apps, underlying network, IPv6 availability or
resolver list rebuild the VPN. Pausing an app needs no new VPN. The engine
passes its traffic through and resumes filtering when the pause expires.
Protected sockets use Android's default underlying network.

The virtual resolver serves UDP and TCP port 53. It uses the owner lookup
before applying host rules and passes allowed or unattributed questions
to the underlying resolvers. UDP replies larger than the TUN payload
budget signal truncation so the client can retry over TCP. TCP port 853
to a virtual resolver is refused immediately for automatic Private DNS.
The resolver keeps pending questions in memory and discards them on a
bounded timeout.

When every chosen app is paused, the VPN may stay up, since the engine
hands every connection on unread, or be taken down to save the forwarding.

## What differs from root

- Connections are made onward by ADBlocker's process: the system's data
  usage counts the chosen apps' traffic under ADBlocker, and the traffic
  does not pass through another VPN.
- Ordinary UDP keeps one protected socket per local endpoint and remembers
  up to 64 recently contacted peers for replies. UDP 443 has a separate queue
  and can use protected sockets connected to the original peer. The engine
  returns port unreachable for recognized QUIC Initials from selected,
  unpaused apps so they can retry over TCP.
- The engine connects to a server before it answers the app in the
  server's place. A server that refuses, or to which there is no way, is
  refused to the app while it still connects, and the app goes on to the
  server's next address; one that does not answer is not answered for.
  Under root the app is answered first, and such a connection is reset
  after it opened.
- For that, a server is connected to before the app has said which host
  it wants: a host the lists block is connected to as well, and the
  connection closed with nothing sent on it. Under root a blocked host is
  never connected to.

## Failure

If `start` fails, or `stopped` is called, the app stops the `VpnService`,
and the chosen apps fall back to the network unfiltered. With the system's
"Block connections without VPN" option, they have no network while the
service is down; the app says so where that option is described.
