# How it works

The root module filters connections from selected apps on the device.
Consider a news app that loads an article and a banner. If the banner
comes from a dedicated advertising host, a domain rule can block the
connection before the module contacts that host.

If the same host serves both requests, the host name alone cannot
distinguish them. With *URL filtering*, which is on by default, the module
can inspect the path inside an HTTPS request. A rule can then block
`/ads/banner` while allowing `/articles/today`. To read the path, the module first
verifies the site's certificate and then establishes a separate encrypted
connection with the app using a locally issued certificate.

Android attributes the forwarded connection to the news app. Its per-app
firewall and VPN rules still apply, and the VPN slot remains available.
On devices [without root](#without-root), a local VPN delivers traffic to
the same filtering engine.

## Bringing the connections to the daemon

Android identifies an app's sockets by its Linux user ID (UID), which also
encodes the profile.
The daemon matches selected UIDs with the kernel's standard packet-owner
match. It combines adjacent selected UIDs into a range. A named shared UID
is one selection, identified by Android's shared user name and the profile
serial. Its current members share the filtering mode, pause, activity and
statistics. When another app joins, the saved group setting applies to it
as well. The daemon resolves the name to its current UID before capture.
Android's network service can update the shared packet filter tables when
a VPN starts or stops.

For a matching TCP packet on port 80 or 443, a rule in mangle `OUTPUT`
sets a routing mark. The kernel routes the packet to the loopback interface
(`lo`). There, TPROXY in `PREROUTING` delivers it to the daemon with the
original destination intact. The local address on the accepted socket tells
the daemon which server the app requested. The rules exist only while at
least one app is selected. Ordinary HTTP/1 traffic on TCP port 80 is
filtered in cleartext. Address rules apply to these captured TCP connections
on ports 80 and 443.

The TCP routing mark depends on a named kernel quota. The rules check its
value without decrementing it, so one switch controls both address families.

The root module rejects outbound UDP 443 from active app UIDs with port
unreachable. Linux can also return `EPERM` to the sending socket
before it reads the ICMP error. The rejection uses the same named quota as
TCP marking and stops when the quota closes. Other UDP ports keep their
Android routes. The daemon does not apply address rules to UDP.

When a user pauses one copy of an app, the daemon can update the UID rules
while the other copy stays protected. It does this after confirming that
the previously active UIDs still belong to the same apps and profiles and
their capture routes are ready. The listeners keep serving connections
while the rule update waits for the packet filter lock. If an identity
becomes uncertain, the daemon disables capture before replacing its rules.

Android rewrites its work profile record during startup. The daemon checks
the new record when Android finishes writing it. An actual profile removal
or stop retires the corresponding UIDs.

TPROXY preserves the destination address for both IPv4 and IPv6, and the
daemon uses the kernel's TCP stack.

## Bringing the lookups to the daemon

Android's resolver sends an app's DNS queries from sockets it assigns to
the app. The packet filter sees these packets as root's, but policy
routing sees the app's UID. For each range of active UIDs, the daemon adds
routing rules for UDP and TCP port 53 that skip packets carrying its
outgoing mark. The same rules catch queries an app sends itself. They lead
to a routing table whose only route is local, through a TUN device that
the daemon holds open and never reads, with `127.0.0.1` as the source (for
IPv6, a private address on the device, added as deprecated so that the
kernel never picks it as the source of other traffic). The query arrives
on the loopback interface, where TPROXY delivers it to the daemon by that
source. The rules take only sockets with no address of their own yet or
with the one the route gives: a socket that already has another address
keeps its own route. Lookups over IPv6 are brought only while IPv6
connections are.

The daemon asks the kernel which UID owns the querying socket and which
network mark it carries. It serves only queries from an app UID to an
address that is not the device's own. A query of an active app is answered
as blocked when the app's connection to the host would be refused before
any request, by the same decision and trusted sites: with URL filtering
off, or for a host whose connections stay encrypted, by the whole-host
rules; otherwise only where no web exception in the lists could apply to
the host. A blocked A or AAAA query gets the unspecified address, and
other blocked questions such as HTTPS get a successful answer with no
records. Both are valid for 0 seconds: Android's resolver caches answers
for all apps of a network together and keeps none with that lifetime, so a
blocked answer reaches no other app and a pause takes effect at once.

Every other query, including one the daemon cannot parse, goes unchanged
to its original server from a socket created with the app's credentials
and network mark, so VPNs and firewall rules see the app. The daemon
relays an answer only when its transaction ID and question match the
query's; for a query it cannot parse, a response with the query's
transaction ID. A TCP connection can carry several queries. The daemon
bounds waiting queries (256, at most 64 for one app), TCP connections (32,
at most 8 for one app), query size (4 KiB), the wait for a server (5
seconds) and an idle connection (10 seconds). At a limit it drops the one
query, and the app's resolver asks again.

Private DNS uses TLS on port 853, and apps' own encrypted DNS uses HTTPS:
neither is filtered here. A name Android's resolver cached for another app
is answered from the cache without a query.

## Connecting on as the app

The daemon creates the outbound socket with the app's credentials and
preserves the bits of its routing mark (fwmark) that Android uses. Data
usage counters, firewall rules and per-app VPN routes therefore see the
original UID. A VPN configured to carry the news app's traffic can route
the forwarded connection through the same tunnel.

The module reserves separate mark bits for incoming and outgoing
connections, outside the bits used by Android and known proxy modules.
The outgoing mark keeps the daemon's connections out of its own listener.

## Deciding from the ClientHello

The first message in a TLS connection, the ClientHello, can contain the
host name. The daemon peeks at this message without consuming it, so it
can still forward the original bytes.

The daemon forwards the original connection when the first TLS record
contains only part of the ClientHello, the name is missing, or the user
has allowed the site. Otherwise, it first checks whether a rule blocks
the whole host. With URL filtering on, the daemon defers the decision
when an exception names the host: the connection is decrypted and its
requests are judged one by one. An exception counts for this only if a
rule blocks the exception's own host whole. One that names no host, or
a host that no rule blocks whole, does not keep a host from being
refused. An exception for named pages counts once the app has made a
request that shows a page. A connection that passes this check stays
encrypted while URL filtering is off.

The host check uses the visible server name (SNI) even when the hello
carries Encrypted Client Hello (ECH). With GREASE ECH, the visible name
is the site's name. With real ECH, it is the provider's public name. The
inner name is encrypted, so domain rules can only match the public name.

With URL filtering on, the daemon also checks the HTTPS exclusions,
whether the app can trust the local certificate, and whether the app has
previously rejected it for this host. Only then does it attempt
decryption. A connection that stays encrypted because its host is on an
exclusion list, has an Extended Validation certificate, or was rejected by
the app is judged by the domain rules, as with URL filtering off: none of
its requests will be seen, so no web exception can apply to them. Where
the app does not trust the local certificate, or the server cannot be
verified, a host that a rule blocks is refused unless such an exception
names it and needs no page. Until
the exclusion lists have been read for the first time, every connection
stays encrypted and is judged the same way.

The daemon completes the server's TLS handshake and verifies its
certificate against Android's system trust store before issuing a
certificate to the app. It offers the app the protocol the server
negotiated (HTTP/2 or HTTP/1.1). If verification fails, the daemon
forwards the original handshake so the app can check the server itself.

A client using real ECH verifies the public name when the daemon answers
without accepting ECH. If it retries with ECH disabled, the new hello
exposes the site's name and is filtered normally. Pin accounting ignores
the `ech_required` alert. A TLS alert rejecting the certificate and
repeated silent closes still count as evidence of pinning. Chrome and
Cronet offer ECH on every connection, so ignoring them for ECH clients
would also ignore apps that pin.

The daemon reports a failed upstream connection to the app. On an inspected
connection, a request for a different site receives HTTP 421 so the app can
reconnect to that site. If inspection permission is withdrawn, the engine
closes the session when the next request arrives.

For connections that pass through encrypted, the kernel uses `splice` to
move data between sockets without copying the payload into the daemon.

## Trust

Most apps trust only Android's system certificate store. Chromium applies
different checks to system and user-installed roots. The module maintains
a local root for each store and a cross-certificate so the chain it
presents can validate against either root.

When URL filtering is enabled, the module mounts read-only copies of both
stores, with the local root added, in the mount namespace of each filtered
personal app process, which is that process's own view of the filesystem.
Android, Settings and other apps see the original stores. The mount goes
away when the process exits.

Apps can cache their trusted roots. The module mounts the stores when a
process starts, before it reads them. If it misses that window, it mounts
the stores on the app's next connection and notifies the app of the change.
The daemon briefly defers decryption and gives the app time to reload its
roots before treating a certificate rejection as evidence of pinning.

The authority is created on the device at first start. Its key never leaves
`/data/adb/adblocker` and is destroyed on uninstall.

The managed profile of the primary user keeps its normal trust stores.
The daemon resolves selected work apps by profile serial and package, then
filters their full socket UIDs. Work HTTPS filtering needs the exported
public CA provisioned in that profile and a client that accepts it.
Upstream TLS configurations are scoped to the profile and exclude its
disabled system roots. See [SECURITY.md](../SECURITY.md#work-profiles).

## What is never decrypted

- Hosts on lists of banks, payment services and other services with
  sensitive personal data. The lists are downloaded like the filter lists.
  The bundled sources match the files used to build AdGuard's Android
  HTTPS exclusions. Internationalized domain names are matched in their
  ASCII form. A wildcard written as a complete label matches one label.
  A usable copy of every bundled exclusion list must be available before
  HTTPS decryption starts. Until then, only domain rules apply. A failed
  refresh leaves usable copies in place.
- Servers that show an Extended Validation certificate, which is known once
  the daemon has connected to the server and before the app is answered.
- Connections of an app whose processes could not be given the mounts.

## Apps that pin

An app can require a particular certificate or public key for a host
(certificate pinning). The daemon detects rejection when the app sends a
certificate alert or repeatedly completes the handshake and closes the
connection before sending a request. Subsequent connections from that app
to that host pass through encrypted. The daemon remembers the app
identity, including its work profile serial, and the host for 30 days,
then checks again.

In root mode, when a process of an app is given the mounts as it starts,
each host remembered for the app gets one try: the next connection of the
app to the host that would pass through as pinned, and that can be
decrypted, is decrypted instead. If the app sends a request on it, the
host is no longer remembered. If the app refuses, or the connection ends
without a request, even one the app opened and never used, the host stays
remembered for another 30 days from then, and the app has lost that one
connection. Its other connections to the host pass through. A connection
that never shows the app our certificate, such as one to a server the
daemon cannot verify, leaves the try for the next. A host refused in a
try is not tried again within a minute, however often the app's processes
start. Lookups go by the pin, and a remembered host that a domain rule
blocks stays refused, without a try. A process given the mounts later, at
a connection, brings no try. Work apps, which are not mounted into, and
VPN mode, where no process start is seen, are checked again only after
the 30 days.

The APK is also a module for LSPosed/Vector (`android/hook`, Xposed API
102), which the user enables for the apps of their choice in
LSPosed/Vector. In those apps it hooks the certificate checks an app adds
to Android's own:

- Network Security Config pin sets
  (`NetworkSecurityTrustManager.checkPins`);
- trust managers an app builds from its own key store, which are Network
  Security Config's (`RootTrustManager`);
- trust managers the app implements, when it hands them to
  `SSLContext.init`;
- OkHttp's `CertificatePinner`, versions 3 to 5, where the app keeps
  OkHttp's class names.

Each check runs first, as the app wrote it, and its result stands unless
it fails. The hook then lets it pass only when Android's default
validation for the app, with the trust anchors of its Network Security
Config, accepts the chain, and the chain's trust anchor is one
of the module's roots in the certificate store of that process, compared
by subject and public key. The hook reads the roots only from a store
that the process's mount table shows to be the module's own copy,
bind-mounted from its data directory on the `/data` file system with
nothing mounted over it. In any other process, including one where
another module copied certificates into the store, the hook lets nothing
pass. Hostname verification is not hooked, and a check that
fails on the site's name keeps failing. Where the check is given the
host, the chain's leaf must name it. Where it is given the handshake's
socket or engine, Android's own trust manager must also accept the chain
for that connection, which checks the name when the app turned on
endpoint identification. An error inside a hook
leaves the app's own result. Pins checked in native code, in an OkHttp the
app renamed, or by the app's code after the handshake still apply. A
trust manager the app wrote is not relaxed when it is called without a
host and the app's Network Security Config has settings for particular
domains. OkHttp's `CertificatePinner` is not relaxed when the app gives
OkHttp a trust manager it wrote.

A host the daemon recorded as pinned before the module was enabled is
decrypted again from the app's next start, as above.

## Filtering

[adblock-rust](https://github.com/brave/adblock-rust) matches requests
against the lists. For a decrypted request the full engine can check its
URL, the page that made it and the kind of resource requested.

The catalog uses uBlock Origin's published filter lists. The engine processes
nested `!#if`, `!#else`, and `!#endif` in each list, including custom rules.
Conditions use `!`, `&&`, `||`, and parentheses. `adguard`,
`adguard_app_android`, `ext_ublock`, `env_mobile`, `env_chromium`, and
`cap_html_filtering` are true. `cap_user_stylesheet`, `false`, and all other
names are false. A malformed condition selects its else branch if present.
`abd rules check` reports unresolved active includes and malformed
conditionals with their line numbers in the checked text. Includes in
custom rules remain unresolved.

When judging an encrypted connection by name, the engine uses a
subset of the rules that can block a whole host, such as `||ads.example^`.
Consider `||example.com^$third-party`: it can block a request to
`example.com` from another site while allowing a direct visit. Using it
on the host name alone would block both. The host-only engine leaves it out.

A top-level document supplies its own first-party context. For other
requests the engine uses the referring page or origin when supplied.
A request that names neither, as most requests of an app outside a page
do, is judged as AdGuard judges one without a referrer: a rule for named
pages (`$domain=news.example`) does not apply to it, whether it blocks or
allows, a rule for every page but some does, and the request counts as
first-party. A browser that withholds its page still says whether the
request is to another site, and that decides the party. The HTTP method,
URL and available resource type still inform the decision.

For plaintext HTTP and decrypted HTTPS, the engine takes the resource type
from `Sec-Fetch-Dest` or a recognized file extension, in that order. A
request whose type is unknown is first checked as `other`. If it goes
upstream and the response's `Content-Type` identifies a resource type, the
engine checks the request again with that type. An HTML response leaves the
request type unchanged.

A rule with `$denyallow=` applies only when the request goes to a host
outside the list of domains in the option. The list covers each domain
and its subdomains. `$domain=` still names the pages the rule is for.
An option made up entirely of underscores is ignored. It can appear
anywhere in a rule's option list.

A blocked request with a matching `$redirect` or `$redirect-rule` that
names a bundled resource gets a local `200` response with that resource.
The engine counts it as blocked. A request blocked without a substitute
gets an empty `403` response. A whole-host block refuses the connection
before the engine reads a request.

A `$replace` rule searches the text of a response with a regular
expression and substitutes the replacement for each match. With URL
filtering on, a matching `$replace` rule takes precedence over
request-level blocks. A matching `$jsonprune` rule removes items from a
JSON or JSONP response and leaves request blocking in effect. A matching
`$hls` rule removes segments from a UTF-8 HLS playlist by URL or tag. It
removes a segment's tags when none of the segments they apply to remain.
If removing a segment would change the next segment's implicit byte
offset, the engine writes that offset explicitly. If it cannot determine
the offset, it forwards the playlist as received.
The engine applies `$jsonprune`, HTML filtering, `$hls`, and `$replace`
in that order, then inserts cosmetic styles and scripts. The inferred
resource type also applies to response rules.
The engine can rewrite an eligible uncompressed text response up to 2 MiB.
It takes these rules from the user's custom rules and catalog lists marked
as trusted for response rules.

A `$removeheader=header-name` rule removes all values of a named response
header. Use `$removeheader=request:header-name` to remove it from the
forwarded request instead. Header names are case-insensitive. The engine
uses the request's type for request headers and the final verdict's type
for response headers. It removes response headers before body rewriting
and cosmetic filtering. For header-only rules, the engine streams the
body as received. Protocol upgrades and `101` responses keep their
headers. Header removal uses the same trusted sources as text response
rewriting.

An exception such as `@@||example.org^$removeheader` disables all header
removals for matching URLs. Adding a value to the exception limits it to
that header and direction.
The `$document` and `$urlblock` page exceptions also disable header removal.

A `$urltransform` rule from the same trusted sources can change the path
and query of an allowed request before it is sent. The engine matches the
changed URL again for blocking and response changes. A rule that changes
the scheme, host, or port of a GET or HEAD request sends the app an empty
`307` redirect with `Cache-Control: no-store`. The app opens any new
connection itself. The engine counts and reports the redirect as a block
and uses the original URL in the recording.

A `$cookie` rule from any list can remove cookies from allowed requests
and responses. Use `$cookie=name` for a case-sensitive cookie name or
`$cookie=/regexp/` for an expression over the name. A bare `$cookie`
selects every cookie. Escape commas and dollar signs in expressions.
The engine removes matching pairs from `Cookie` before sending the request
and matching `Set-Cookie` fields from the response head.

Add `;maxAge=seconds` to cap the remaining lifetime of matching
`Set-Cookie` fields. The engine uses the server's `Max-Age` or, when
absent, the time until a valid `Expires`. It removes `Expires` and writes
the capped `Max-Age`. Expired cookies receive zero seconds. Session
cookies receive the rule's limit, floored at zero. Add `;sameSite=strategy`
to set `SameSite` to `Strict`, `Lax` or `None`. Rules with either
attribute leave request cookies alone. The engine preserves malformed
cookie fields and streams the body as received. `@@...$cookie` exempts all
cookies.
`@@...$cookie=name` and `@@...$cookie=/regexp/` exempt selected names.
The `$document` and `$urlblock` page exceptions also disable cookie rules.

A `$removeparam=name` rule strips populated values of a case-sensitive
query parameter on GET, HEAD, OPTIONS and POST requests. Any list can
supply one. Without a resource type qualifier the rule applies to
documents. Names can use ASCII letters, digits, hyphens and underscores.
`@@...$removeparam=name` preserves that parameter. The `$document` and
`$urlblock` exceptions preserve the query. The engine applies matching
URL transformations before matching and removing parameters. It keeps
same-origin changes on the existing connection and judges the resulting
URL before forwarding.
The original URL appears in a live recording.

With URL filtering on, any list can use `$csp=policy` to add a Content
Security Policy to an allowed document or frame. If the request type is
unknown, the policy is added when the response identifies itself as HTML.
For example, `||example.org^$csp=frame-src 'none'` blocks every frame on the
page. A rule with `$subdocument` applies only to frames. Matching policies
are appended in one response header field alongside the server's policies.
`@@...$csp` disables all matching policies and `@@...$csp=policy` disables
the named policy. The `$document` and `$urlblock` exceptions also disable
the list policies. The engine rejects reporting directives. When cosmetic
filtering adds a style or script, it handles each list policy in the same
way as a server policy.

A rule can name the apps it is for with AdGuard's `$app=` option.
Entries separated by `|` can be package names, prefixes ending in `*`,
or regular expressions enclosed in `/`. Names and prefixes ignore ASCII
case. Regular expressions search the package name as written and are
case-sensitive by default. Escape `/`, `$`, `,`, and `|` in an expression
with a backslash. Prefix every entry with `~` to exclude matching apps.
`||ads.example^$app=com.reader` blocks the host in that app only, and
`$app=~com.reader` in every app but that one. An exception with the
option allows in the named apps what a rule for every app blocks. These
rules apply to lookups, to connections that stay encrypted and to
decrypted requests alike. A blocking rule that names both an app and a
page is left out. A response rule can match both.
The apps of a shared UID are not told apart, and no rule for apps applies
to them.

AdGuard's `$network` rules match the remote IP address and optionally its
port. Exact addresses use brackets for IPv6 and accept a leading `|` or
`||` and a trailing `^`. Slash-delimited regular expressions match either
the address alone or the address with its port. For example, an IPv6
endpoint is searched as `[2001:db8::1]` and `[2001:db8::1]:443`. A rule may
also use `$app` and `$important`. An important block wins over an
exception, which otherwise bypasses further filtering. The engine checks
the address before reading from the app, including when URL filtering is
off. DNS queries still use the host rules.

On TCP port 80 the engine recognizes a bounded HTTP/1 request header.
With URL filtering off it judges the Host header. With it on, each ordinary
request on the connection is checked against the web rules. Blocked
requests are answered locally. With root, the module connects to the original
destination, with the original app's identity, only when it has a request
to forward, then reuses that connection. The VPN has already connected
before it answered the app (see below). Unsupported initial
protocols pass through unchanged.

The shared engine applies cosmetic rules to decrypted HTML by default in
both root and VPN modes. **Hide page elements** can be turned off in Settings.
The engine puts site CSS and a single script right after the
page's `<head>` tag. The script runs matched uBlock Origin scriptlets and
AdGuard JavaScript rules and hands matched procedural actions to a throttled
DOM observer.
The rule reader maps compatible AdGuard `#%#//scriptlet(...)` calls and
their exceptions to the bundled scriptlets. It maps single CSS rule sets
with `#$#` or `#$?#` and their exceptions to style and removal actions.
AdGuard `#%#` JavaScript rules and `#@%#` exceptions require named domains
and come from marked lists or user rules. `$jsinject` disables scriptlets
and JavaScript rules on matching pages.
AdGuard `$$` rules and compatible uBlock Origin `##^` rules cut elements
out of the HTML on the document's own host in decrypted HTTPS and
eligible plaintext HTTP. They can match a tag, its
attributes, its inner HTML, and a chain of direct parents. The engine
requests an uncompressed body when a rule applies to the host. It also
checks responses labeled `text/html` or `application/xhtml+xml` when
the request type is unknown. `$@$` and `#@#^` exceptions cancel the same
selector on the domains they name. `$content` and `$document` exceptions
disable HTML filtering for the matching page.
Selectors such as `:has()` stay in native CSS where the parser represents
them as CSS. Rules based on generic classes and IDs can still use a second
style at the end of the document. The cache keeps compiled permissions,
JavaScript rule bodies, and parsed HTML rules. See
[SECURITY.md](../SECURITY.md#hiding-page-elements) for the CSP and trust
limits.

## Failing open

- The daemon starts listening and prepares both the rules and the capture
  routes before enabling the quota. If it stops, the supervisor writes zero
  to the quota through `/proc/net/xt_quota/`, which stops TCP marking at
  once. New connections then follow Android's routing and VPN policy even
  while another process holds the xtables lock.
- A packet already marked for capture may leave only through loopback. If
  its routing rule disappears, the packet filter drops it before it can
  reach the underlying network.
- The lookup routes exist only while the daemon's process holds its TUN
  device. If the daemon exits for any reason, the device and its routes go
  with it, and lookups follow Android's routing at once, before the
  supervisor removes the rules. Cleanup removes the lookup routing rules
  before waiting for the packet filter lock. If lookup capture cannot be
  set up, the daemon filters connections without it. If an answer cannot
  be sent from the lookup's server address, as when another program
  holds UDP port 53 on every address of the device, the daemon closes the
  TUN device and says so in its log; lookups then follow Android's
  routing.
- If server verification fails or the app rejects the local certificate
  for a host, the app can establish its own TLS connection to the server.
- When an IPv6 route is unusable, accepting the app's socket locally can
  prevent the app from retrying over IPv4. The daemon temporarily suspends
  IPv6 capture when it detects this condition, allowing the app's own
  `connect()` to report the failure.

## Installing the root module

The module is a set of scripts and the daemon for the device, taken from
the app's APK by its installer (`module/customize.sh`). It has no system
overlay, SELinux rules or `post-fs-data` script, so root alone can install
it, and it can be in either of two places.

- The app installs it with `module/direct.sh` into
  `/data/adb/adblocker_direct` and starts it at once, when onboarding or
  **Settings › Set up root mode** sets root mode up. No root manager lists
  it. A launcher in `/data/adb/service.d`, which KernelSU, Magisk and APatch
  run at late_start, starts it at boot. Where nothing runs that directory,
  the app starts the daemon when the device has finished booting, which
  takes root being granted to it then. The app asks for root at boot only
  in root mode with a module it installed, and does nothing when the daemon
  runs already or the module turned itself off. It does the same when it
  is opened, once, for a device that did not deliver the boot broadcast.
- Flashed in a root manager, it is in `/data/adb/modules/adblocker`. This
  one is in use once it has its daemon: a manager keeps a directory without
  it for a module it applies at the next boot. When it first starts, it
  stops the daemon of a module the app had installed and removes that
  module. Installing from the app then brings the manager's module up to
  date where it is, without a restart.
- An update of the app carries a new module. When the app is next opened
  in root mode it replaces an older module with its own, wherever that one
  is, unless the module is turned off, marked for removal or waiting in a
  root manager for a restart. A newer module is left alone. Meanwhile the
  overview says that root mode is updating. A module left older, as after a
  failed update, is offered for update in Settings.
- A module flashed in Magisk or APatch waits for a restart. Opened before
  it, the app sets up its own copy, which filters until the manager's takes
  over at the restart. Where the manager has an update waiting for a module
  that is in use, or has marked it for removal, the app installs nothing
  over it: that module goes on until the restart.
- Both keep their data in `/data/adb/adblocker`, so the selection, the
  lists and the authority carry over from one to the other.
- The app removes a module it installed, from **Settings › Remove root
  mode**: the daemon stops, apps stop
  trusting the authority, its key is destroyed and the data is deleted.
  Copies of the certificate stores that running apps still have mounted
  are deleted at the next boot, where a root manager runs
  `/data/adb/service.d`. A module in a root manager is removed there, as
  Settings says in its place. With root mode removed, Settings offers to
  set it up again or to filter in VPN mode instead.
- With the app uninstalled, nothing is left to control a module it
  installed. Its daemon reads Android's package list when the list changes
  and ends, removing its rules, once the app has been absent for 30
  seconds; the data stays for the app installed again. At the next boot the
  launcher removes the module and its data if the app is still absent. A
  package list that cannot be read, or that lacks a package every device
  has, changes nothing.

## Without root

On devices without root, the Android app runs the filtering engine in its
own process and receives packets through a local VPN. The app and engine
communicate through the [JNI interface](engine-jni.md).

- The app registers packages with `addAllowedApplication`. For All apps it
  fills the allowlist from the current personal app UID inventory, omitting
  ADBlocker and excluded UIDs. The package listener refreshes the inventory
  and rebuilds the tunnel when this set changes. This keeps non-app UIDs
  outside the tunnel even when they have no package Android could deny.
  An empty allowlist leaves the tunnel down.
- The engine is built as a native library, `libabd_vpn.so` (`abd-vpn`), and
  runs in the app's process. A TCP stack in user space turns the VPN's
  packets into connections. Android identifies the originating app through
  `getConnectionOwnerUid`. The engine applies the same filtering rules as
  the root daemon. Outbound sockets bypass the local VPN to avoid a loop.
- The engine checks address rules before opening a TCP connection to the
  server. It connects to eligible servers before accepting the app's
  connection. If the server is unreachable, the app can try another
  address. The host name arrives after the TCP connection opens. If a rule
  blocks the host, the engine closes the connection without sending
  application data. DNS filtering usually keeps an app from reaching a
  blocked host's address in the first place.
- The VPN carries IPv6 only when its underlying network supports it.
  Otherwise Android can reject an IPv6 connection immediately.
- Nothing is decrypted unless the user has installed the authority's
  certificate in the user store and URL filtering is on. Apps that trust
  only the system's certificates are filtered by host name.
- For a selected, unpaused app, a recognized QUIC Initial receives an
  ICMP port-unreachable response even when it uses an alternative port.
  Clients that retry over TCP use the ordinary HTTPS path. The relay also
  checks address rules for ordinary UDP. It drops blocked datagrams and
  sends ICMP port-unreachable within its reply limit. An address exception
  bypasses QUIC refusal. Queries sent to the virtual resolver use the
  DNS path.
- If the engine cannot start or stops, or another VPN app takes the VPN,
  the VPN service ends. Apps regain direct network access unless Android
  is configured to block connections without the VPN.

### DNS in the VPN

Android sends DNS to a virtual resolver in the VPN subnet. The engine
handles UDP and TCP port 53 there, using Android's socket owner lookup.
It uses the same host rules and trusted-site exceptions as for visible
TLS names. A or AAAA blocks get a 30-second zero-address answer. HTTPS,
SVCB and other blocked questions get NOERROR with no records. Each allowed
query goes to a resolver from the network's link properties through a
protected socket. The engine checks the transaction ID and question
before relaying the reply. If a resolver fails, it tries the next one.

The engine resets TCP 853 to the virtual resolver, so Android's automatic
Private DNS probes can retry in plain DNS. Strict Private DNS and app DoH
can bypass this filter.

## Switching to root

While the app prepares a switch from VPN, the module keeps a private
checkpoint of its subscriptions, custom rules and trusted sites. Canceling
the switch restores those settings and removes downloaded copies of lists
that are no longer enabled. The app schedules an update for restored
subscriptions that need to be downloaded again. Completing the switch
discards the checkpoint.

Settings switches in one action: where no module of the app's version is
in use, it sets root mode up first, then hands over. A module in use that
a root manager updates or removes at the next restart cannot be replaced
before it: Settings then offers no switch and says to restart first, and
in root mode it shows that module as running, with what the restart
does. A switch that does not finish stays pending, and trying it again in
Settings does the same.
Canceling it leaves root mode set up; Settings shows it under the method,
with its removal, while the VPN is in use.

## Cost

- The daemon waits for connections and configuration changes. The app
  schedules subscription updates through WorkManager.
- A subscription download follows active `!#include` directives on the
  same HTTPS origin and within the including file's directory. It stores
  their rules in place in the list. Updates check validators for the list
  and its included files. If any required file fails to download or pass
  validation, the previous complete copy stays in use.
- When a catalog list moves, its saved selection follows the new address.
  An enabled list keeps filtering with the rules and trust of its previous
  copy until an update validates and publishes the replacement. A failed
  download is reported while the previous copy remains in use. Turning
  the list off parks its previous copy under the same cache limit as other
  disabled lists. A targeted update can replace that parked copy while the
  list is disabled. Publishing a replacement removes the previous copy
  and its download metadata.
- The daemon resumes TLS sessions on both sides, so the many connections
  a browser opens to one site do not each cost two full handshakes.
- On startup, the daemon can load compiled lists from its cache.
- Enabled subscriptions and custom rules share a 64 MiB input budget. An
  update that would exceed it reports an error for the list and keeps
  its previous copy. A generation that exceeds the budget is rejected
  as a whole.
- Turning off a downloaded subscription takes its rules out of the engine.
  The engine can keep up to 32 MiB of disabled lists on disk so that
  turning them back on can reuse the last download.
  Enabling a list without a local copy downloads that list alone.
- The kernel forwards connections that pass through encrypted. For
  decrypted requests and responses, the engine streams the body as it
  arrives.

Security considerations are in [SECURITY.md](../SECURITY.md).
