# Security

With **URL filtering** off, ADBlocker can check the remote address and
visible host name of an HTTPS connection. The request path and contents
stay encrypted. With **URL filtering** on, ADBlocker decrypts eligible
connections of every filtered app on the device and inspects individual
requests.

ADBlocker refuses QUIC from selected apps at once, so they can retry over
TCP, where the usual HTTPS inspection and certificate checks apply. The
root module refuses UDP 443 for selected apps that are not paused. The VPN
refuses the first packet of a recognized QUIC connection on any UDP port.
An address exception bypasses this refusal in VPN mode. Apps that require
QUIC may fail to connect.

A list can block a connection by remote address and optional port, or let it pass
without inspection. The VPN applies these rules to TCP and UDP. Root mode
applies them to captured TCP on ports 80 and 443. DNS queries handled by
ADBlocker are judged by host name.

## Reporting a vulnerability

Use **Report a vulnerability** on the repository's **Security** tab to
submit a private report. Include the ADBlocker version from `module.prop`,
the device model, the Android version, and the steps to reproduce the issue.

## What using it changes

URL filtering is on by default. When ADBlocker can decrypt
an HTTPS connection, it first verifies the site's certificate against
Android's system certificates. It then presents the app with a
certificate for that site, signed by its local certificate authority
(CA), checks each request against the filter rules, and forwards allowed
requests over its own encrypted connection to the site.

With URL filtering on, a filter list can select a bundled resource for the
engine to return in place of a blocked request's response. The list only
names a resource. Its content is fixed in the build. HTML substitutes
are sandboxed and cannot load external resources.

For recognized cleartext HTTP on TCP port 80, both filtering methods answer
blocked requests locally. With root, the module opens a connection to the
original destination only when it has a request to forward, so a server
whose requests are all blocked is never contacted. The VPN connects to the
server before it answers the app, as for any TCP connection, and sends
nothing on it for a blocked request.

**Chosen apps** is the default. **All apps** extends filtering to
installed personal apps with attributable app UIDs, including newly
installed apps. ADBlocker, excluded UIDs and system UIDs are outside
filtering. A named shared UID is one entry, so excluding it covers every
member. Work apps still require an explicit selection.

The filtering method determines which apps can see or trust the local CA.

- **Root mode, personal apps.** While URL filtering is on, the module adds
  its CA to a private copy of the certificate stores that only the
  processes of filtered personal apps see. Android and apps outside the
  scope keep their original stores. In All apps, a personal app can
  detect its own mounted store once URL filtering is on.
- **Root mode, work apps.** The administrator can provision the public CA
  in the work profile. See [work profiles](#work-profiles).
- **VPN mode.** If you set up HTTPS filtering, Android adds the certificate
  to your user certificate store. Any app that trusts user certificates
  can then trust ADBlocker's, even when it is excluded. An excluded app can
  also observe Android's VPN state. Android also routes the SDK sandbox
  processes of the packages the VPN allows into the VPN; the engine admits
  a connection only when Android attributes it to an app UID in scope.
  Turning off URL filtering stops inspection. To remove the trust as well,
  follow **Settings › Remove the certificate**.

### Limits of HTTPS inspection

- The browser sees ADBlocker's certificate for the site. Chromium skips
  Certificate Transparency (the public record of issued certificates) and
  its built-in certificate pins for locally trusted authorities.
  ADBlocker verifies the server's certificate chain, but does not check
  those logs, pins or certificate revocation. It can accept a wrongly
  issued certificate that the browser would otherwise have rejected.
- With URL filtering off, connections using Encrypted Client Hello (ECH)
  stay encrypted. Domain rules can match the visible server name. For real
  ECH, that is a public name that may stand for several sites, and the
  individual site's name remains hidden.
- With URL filtering on, ADBlocker can answer an eligible ECH handshake
  after verifying the upstream certificate for the visible name. For
  real ECH, the client authenticates the public name and sees ECH rejected
  without a replacement configuration. Clients that retry with ECH
  disabled expose the site's name on the new connection. Filtered apps
  lose ECH privacy for these decrypted connections. Clients that do not
  retry may report a connection failure.
- Some apps accept only a specific certificate or public key for a site.
  This is called certificate pinning. When ADBlocker detects it, later
  connections from that app to that site are filtered by domain only. In
  root mode, ADBlocker tries one connection to such a site again when the
  app starts, and filters the site by URL if the app accepts it. An app
  that still pins loses that one connection. A site that a domain rule
  blocks stays blocked and is not tried.
- In root mode with LSPosed, enabling ADBlocker's module for an app makes
  that app's own certificate checks accept a chain that ends in
  ADBlocker's local CA for this device, once Android has validated it as
  for any connection of the app. The app then no longer refuses the
  decryption of the sites it pins, which are filtered like other sites,
  with the limits above. For those apps, the app's pins are replaced by
  ADBlocker's verification of the server against Android's system
  certificates: a server certificate that the app's pins would reject is
  accepted when it chains to any CA Android trusts. Chains that end in
  any other CA meet the app's checks unchanged. A check that fails
  because the certificate does not name the site keeps failing. The
  module runs only in the apps you enable in LSPosed, and accepts the
  local CA only where the module has mounted it into the app.

Connections to hosts on the exclusion lists for banks, payment services
and other sensitive sites stay encrypted, as do connections to servers
with an Extended Validation certificate. The lists come from AdGuard
and do not cover every sensitive site. Updates can add entries.
ADBlocker keeps existing protections when an update omits an entry or a
download fails.

### Changing requests and text responses

With URL filtering on, ADBlocker can rewrite uncompressed text responses
up to 2 MiB in decrypted HTTPS and plaintext HTTP traffic. Only the user's
custom rules and the AdGuard and uBlock Origin lists marked for response
rewriting in the subscription catalog can supply `$replace`,
`$jsonprune`, `$hls`, `$removeheader` and `$urltransform` rules. A matching
`$replace` rule can override a request block and change text the app
displays or code it runs. A matching `$jsonprune` rule removes items from
a JSON or JSONP response and leaves request blocking in effect.
A matching `$hls` rule removes segments and their associated tags from an
UTF-8 HLS playlist and leaves request blocking in effect.
A matching `$removeheader` rule can remove an HTTP request or response
header. The engine rejects rules for the headers protected by AdGuard
and the additional headers listed for each direction.
[Protected headers](abd-engine/src/rule/action/removeheader.rs)
A matching `$urltransform` rule can change an allowed request's path
and query before it is sent. It can redirect a GET or HEAD request to
another address. The app decides whether to follow the redirect and
opens any new connection itself.
Whole-host refusals happen before the engine reads a request. The engine
applies response rules before it adds cosmetic styles or scripts.

### Hiding page elements

**Hide page elements** is on by default. ADBlocker can change whole,
uncompressed HTML documents it already decrypts. You can turn it off in
Settings or with `abd cosmetic off`.
A list can hide legitimate content, change the DOM, or alter page behavior.

- Any list can cut matching elements out of the HTML on the pages it
  names. This can remove page content, scripts, and HTML policy elements.
  The engine reads at most 2 MiB for this edit. It skips encodings it cannot
  scan, including UTF-16 and UTF-32, and skips elements beyond its nesting
  limit. The original body passes through if the element, decoding, or
  time limit is exceeded.
- ADBlocker injects selectors as CSS and runs matching functions from a
  bundled uBlock Origin scriptlet library in the page's JavaScript context.
  Compatible AdGuard scriptlet rules use the same library and permissions.
  ADBlocker passes rule arguments as data. A page can detect the additions and
  observe the effects of scriptlets. Scriptlets have the page's privileges
  and follow its policies for network requests and dynamic code evaluation.
- A list marked `trustedScripts` and the user's own rules can run JavaScript
  of their choice in the pages they name, with the privileges of those pages.
  The catalog grants this mark to the Android builds of AdGuard Base,
  AdGuard Chinese and AdGuard Mobile Ads. Other lists cannot supply
  JavaScript rule bodies.
- For procedural filters, a bundled script applies selectors and actions
  such as text matching, removing elements and setting CSS properties.
  Supported AdGuard CSS injection rules use these same style and removal
  actions.
  It rechecks DOM changes at most once per 100 ms, caps work and tracked
  elements, and stops observing on `pagehide`. It may skip expensive
  selectors and patterns, and leaves out styles that can refer to external
  resources.
- Where an enforced Content Security Policy restricts inline scripts,
  ADBlocker adds a SHA-256 hash for the exact script in each applicable
  response header. The script goes before any CSP meta tags, which apply
  to subsequent content. The added hash applies to script elements only.
  ADBlocker keeps the policy's existing source expressions and directives,
  including `'strict-dynamic'`, adds no nonce or permission for inline
  handlers or dynamic code evaluation, and leaves report-only policies
  unchanged. For CSS, ADBlocker adds a hash for the style it puts in
  the head. A style added at the end of the page carries the page's own
  style nonce when the policy requires one.
- ADBlocker skips unsafe scriptlet calls and JavaScript rule bodies, and
  procedural actions larger than 4 KiB. It accepts JavaScript rule bodies
  up to 16 KiB and keeps up to 4,096 distinct bodies totaling up to 4 MiB
  per filter generation. It caps the page
  script at 256 KiB, gives whole scriptlet and JavaScript calls priority,
  and keeps up to 128 procedural actions in sorted order. ADBlocker omits
  the script for response policies that require Trusted Types or forbid
  scripts in a sandbox, and when the injection point, encoding or policy
  is ambiguous. CSS can still apply when the page opens with an explicit
  `<head>` tag. A page passes through unchanged when that tag does not
  appear in the first 16 KiB, preceded only by white space, comments, the
  document type and the `<html>` tag.
- Privileged resources, including trusted aliases and dependencies, require
  a direct HTTPS download from an exact catalog URL marked
  `trustedScriptlets`. The catalog grants this permission to uBlock
  Origin's own filter lists and the Android builds of AdGuard Base,
  AdGuard Chinese and AdGuard Mobile Ads. Custom subscriptions and user
  rules can use ordinary scriptlets. The `trustedScripts` mark uses the
  same direct HTTPS download and exact URL requirements. Redirected
  downloads never receive trusted permissions.
- For injection, the engine requests uncompressed documents and reads at
  most 16 KiB ahead for up to 250 ms to inspect the explicit head. It
  streams the remaining document after any whole-body edits. It records
  neither the selected scripts nor their page URLs.

## DNS in root mode

The daemon receives the plain DNS lookups, over UDP and TCP port 53, of
filtered apps that are not paused, whether Android's resolver or the app
sends them. Routing rules select only those apps' UIDs: lookups of other
apps, of the system and of root are not routed to it. It answers a lookup
the rules block with a zero address, or no records, valid for 0 seconds,
so Android's shared resolver cache does not give the answer to another
app. Every other lookup goes unchanged to the server the app chose, as the
app and on its network; the daemon relays the reply only when its
transaction ID and question match the query's, or, for a query it cannot
parse, when it is a response with the query's transaction ID. ADBlocker
does not store query names. Stats count blocked lookups as blocked hosts,
and Activity shows a blocked name only during a live recording.

Private DNS and an app's own encrypted DNS, including DoH, bypass DNS
filtering, and so does a socket an app bound to an address of its own or
connected before its lookups were brought to the daemon. A socket that
took the daemon's loopback source keeps it after the daemon stops, and its
queries go unanswered until the app closes it. Android can answer a
filtered app from its cache with an answer it received for another app.
With URL filtering on, a lookup of a host whose requests can be read is
blocked only when no web exception in the lists could apply to it; one
whose connections stay encrypted is judged by the domain rules. A
connection to the host is still checked against the rules.

## Shared UIDs in root mode

Android assigns sockets to a UID. When several apps in a profile share
that UID, choosing a member enables the whole named group, including
members Android installs later. Pausing or removing the group applies to
all its members. The daemon uses the profile's saved group identity to
attribute each connection once. For groups, the engine respects every
package-specific HTTPS exclusion for the destination because the socket
does not identify an individual member.

## Work profiles

The same package installed personally and at work has separate selections,
network identities, pauses, recordings and counts. Root mode associates each
work selection with the profile serial assigned by Android. An installation
in a replacement profile requires a new selection.

Work apps use the profile's existing certificate stores and their own trust
policy. For HTTPS filtering the profile administrator can install the public
certificate exported by ADBlocker. Any work app that accepts user certificates
may then trust that CA, including apps outside the selection. Only selected
connections are routed to the daemon. App certificate pinning still applies.

The daemon verifies upstream servers using system roots and respects the
system roots disabled in that profile. Removing the public CA in the work
profile withdraws its trust for new TLS handshakes. Removing ADBlocker
destroys the private key on the device, but certificates the administrator
installed stay in the work profile until the administrator removes them.

## VPN mode

Android sends only the selected apps' connections through the local VPN.
Installing a certificate is a separate choice.

- Only you can install the certificate, through Android's settings. Android
  displays a network monitoring notice for as long as it is installed.
  Most apps trust only system certificates and still get domain filtering.
- Anyone who obtains the authority's private key can impersonate a site to
  apps that trust the user certificate if they can intercept the connection,
  even on another network or while ADBlocker is stopped.
- The private key stays in the app's private storage, is excluded from
  backups, and is deleted on uninstall. The CA certificate remains in
  Android's user store until removed through system settings.
- The engine verifies upstream servers using Android's system roots.
  User-installed roots, including ADBlocker's, do not participate in this
  check.
- The app uses its network permission to forward allowed connections and
  download lists. In root mode, the module does both instead.
- The filtered apps' traffic leaves the device from ADBlocker's process. The
  system's data usage counts it under ADBlocker, and it does not pass
  through another VPN.
- The VPN engine checks address rules before connecting, then opens an
  allowed TCP connection before the app sends the host name. If a rule
  blocks that host, the engine closes the connection without sending
  application data. The server can see the device's IP address and the
  connection attempt. With root, a connection
  to a host that a rule blocks outright is refused before the module
  connects to the server.

### DNS in the VPN

The VPN receives plain DNS addressed to its virtual resolver. It applies
domain rules only when Android attributes the query to a selected,
unpaused app. A blocked address query gets a zero address with a 30-second
TTL. Other blocked questions receive a successful answer without records.
Allowed queries go to the underlying network's DNS servers. ADBlocker
does not store query names. Stats keep counts, and Activity shows a blocked
name only during a live recording.

The virtual resolver refuses DNS over TLS for Android's automatic Private
DNS probes. Strict Private DNS and an app's own encrypted DNS, including
DoH, bypass DNS filtering. Android and apps can reuse cached DNS answers
until their TTL expires. ECH can also hide a domain from the separate TLS
filter. Certificate pinning limits request inspection even after a
connection falls back to TCP.

## What protects the device

- In root mode, the authority's key is created on the device, stays in
  `/data/adb/adblocker` (readable by root only) and is destroyed when root
  mode is removed. Where the app set up root mode and is uninstalled
  without removing it first, the key is destroyed at the next restart if
  KernelSU, Magisk or APatch is installed, and stays with root from
  another source. In the personal profile, only the processes of filtered apps
  see the local CA. Work apps use the certificates provisioned in their
  profile.
- The LSPosed module recognizes the local CA by its key, as found in the
  certificate store the module mounted into the app. It reads that key
  only from a store that the app's mount table shows to be the module's
  own copy from `/data/adb/adblocker`, which only root can write to. A
  certificate that only carries ADBlocker's name, also one that another
  module copies into the system store, does not pass the app's checks in
  its place.
- The daemon carries only connections and lookups the rules delivered to
  it for an app, and makes onward connections and queries only as that
  app. In VPN mode, Android routes only eligible apps in scope into the
  VPN and attributes each connection to its app before the engine filters
  it.
- The engine limits concurrent connections, retained state and filter list
  input. Timeouts bound how long it waits for an app or a server.
- Filter lists are fetched over HTTPS. They select what is blocked and,
  while **Hide page elements** is enabled, what is hidden or removed and
  which scriptlets and trusted JavaScript rules run in decrypted pages, as described in
  [hiding page elements](#hiding-page-elements).
- When the daemon stops, its lookup routes go with its process, and
  cleanup removes the lookup routing rules and disables TCP marking and
  UDP rejection before waiting for the packet filter lock, so new
  connections follow Android's routing and VPN lockdown policy. If cleanup
  cannot remove the rules after retrying, the supervisor reports the
  failure and stops. In VPN mode, the VPN goes down when the engine stops;
  with the system's "Block connections without VPN", the selected apps
  then have no network until it reconnects.
- By default, logs omit the destinations that apps connect to. ADBlocker
  sends no telemetry. It downloads subscribed lists and forwards allowed
  traffic to its destination.

## Scope

Reports are in scope when an unprivileged app, a server, a network
participant, or a filter list can obtain the CA private key or cause the
engine to violate the guarantees described here. An attacker who already
has root access on the device is outside this threat model.
