# Contributing

## Building

`./scripts/build.sh` builds the daemon and the engine library
(`libabd_vpn.so`, for VPN mode) for arm64 and x86_64, then the app, whose
APK carries the daemons and the module scripts at its root and the library
in `lib/`, and copies it to `out/adblocker-<version>.apk.zip`. Without the
library the app builds, and reports VPN mode unavailable. It needs:

- Rust stable with the targets `aarch64-linux-android` and
  `x86_64-linux-android`
- the Android SDK (`$ANDROID_HOME`, or `~/Android/Sdk`) with an NDK
  (`$ANDROID_NDK_HOME`, or the newest in the SDK)
- JDK 21, for Gradle

A release key is read from `android/keystore.properties` (`storeFile`,
`storePassword`, `keyAlias`, `keyPassword`); without it the APK is signed
with the debug key.

## Checks

CI (`.github/workflows/ci.yml`) runs these checks. Run them before you
open a pull request:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check
sh scripts/check-engine-jni.sh
for s in module/*.sh scripts/*.sh scripts/check/*.sh scripts/check/http3/*.sh; do bash -n "$s"; done
sh scripts/check-module-uninstall.sh
sh scripts/check-module-boot.sh
if command -v actionlint >/dev/null; then actionlint; fi
./scripts/build.sh --locked
cd android
./gradlew spotlessCheck :build-logic:convention:spotlessCheck
./gradlew test
./gradlew lintStructure -PlintStructureOnly
./gradlew :app:dependencyGuard :app:checkReleaseBadging :app:licenseeAndroidRelease :app:lintRelease
```

The app's screenshot tests compare with the images checked in next to
them; after a change to the interface, `./gradlew recordRoborazziDebug`
records them again. More in [android/AGENTS.md](android/AGENTS.md).

The code structure limits are 800 physical lines per production source file,
500 for a file with Compose functions, and 80 per function. A physical
function span includes blank lines and comments between the declaration and
its end. Clippy uses its own count for Rust and omits blank lines and
comments, so an 80-line Clippy limit permits some longer physical spans.
Clippy checks test code too. A test or fixture that needs more room may
allow `clippy::too_many_lines` on that function with a reason.
`cargo test -p abd --test source_size` checks the 800-line file limit
across the Rust workspace sources, excluding test-only modules and items.

`cargo test -p abd --test module_cycles` checks production Rust modules
for cycles between siblings in each crate. It follows imports, named
re-exports and qualified paths, and reports a shortest cycle with its
source locations. It checks the host and both Android targets separately
using item-level OS and architecture conditions. Macro expansions and
individual names from wildcard imports are outside the source analysis.

Android lint checks the Kotlin limits in the main sources of application,
library and JVM modules that use the lint convention. AGP provides no lint
task for standalone test APKs such as `:benchmarks`. Lint counts named
functions from `fun` through the end of their body and applies the 500-line
file limit when a named function has a Compose `@Composable` annotation.
Separate test source sets and generated code are outside these limits. Run
the detector tests with `./gradlew :lint:test` from `android/`.

`config/filter-catalog.json` supplies both the Android subscription
catalog and the engine's standalone defaults. Keep subscription IDs stable
and put regional suggestions in `regions`. The engine uses only
`defaultEnabled` entries when no subscription configuration exists.

`cargo deny` is installed with `cargo install --locked cargo-deny`; what it
holds the dependencies to is in `deny.toml`.

The tests run without root and without a device. Those of the connection
path (`abd-engine/src/mitm/tests.rs`) start a TLS server and a daemon in the test process
and are clients of both.

### HTTP/3 fixture

The `abd-vpn` example runs a local HTTP/3 server and a client that offers
only `h3`. Build it with `cargo build --locked -p abd-vpn --example check_http3`.
In a temporary directory, create a one-day certificate for the fixture:

```sh
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -subj /CN=fixture.example -addext subjectAltName=DNS:fixture.example \
  -addext basicConstraints=critical,CA:FALSE -keyout key.pem -out cert.pem
openssl x509 -in cert.pem -outform DER -out cert.der
openssl pkcs8 -topk8 -nocrypt -in key.pem -outform DER -out key.der
```

Use the binary in `target/debug/examples`. From the directory containing
the certificate, start `check_http3 origin 127.0.0.1:14443 cert.der key.der`.
In a second terminal run `check_http3 fetch 127.0.0.1:14443 cert.der`.
The client prints the status of two concurrent requests on one connection.
The server prints each path it received. Two 200 responses establish
passthrough. A path-filtering test must also check the origin received
only the allowed request. Run `check_http3 check-rules cert.der key.der` to
exercise this case with two local endpoints and the shared request policy.
The same case runs in `cargo test` with a temporary certificate.
For an Android filtering check, expose the origin
on UDP 443 and run the client under the selected app's UID.

On a root-capable test emulator, build the test binary with
`./scripts/build-http3-check.sh`. Pass `arm64-v8a` for an arm64 emulator.
After installing the module, run `./scripts/check-http3-device.sh root`.
With the debug app and instrumentation APKs installed and VPN consent
granted, run `./scripts/check-http3-device.sh vpn`. In each check, the
selected app must receive port unreachable within one second, get a 403
for the banner and a 200 for the article over TCP. The unselected control
app receives both responses over HTTP/3. Both checks create an isolated
origin on a veth pair and remove their fixture on exit. The VPN checks
IPv6 filtering when its underlying network has an IPv6 route.

## On a device

The packet filter rules, capture routes, mounts of the certificate
store and the module scripts are only exercised on a device. On an emulator
with a `google_apis` image, which allows `adb root`:

```sh
./scripts/install-on-emulator.sh
./scripts/check-device.sh
```

`check-device.sh` builds a small app to be the client, and needs a JDK and
the Android build tools and a platform in the SDK. It expects the AOSP
iptables lock file. On the emulator the check also creates a
disposable managed profile, provisions its public test CA through Android's
device policy API, and verifies work and personal filtering independently.
The device check installs apps that share a UID and sends requests from
each member to verify that filtering and pausing follow the group as its
membership changes.
It also checks that All apps filters a newly installed app and leaves excluded
apps and system UIDs outside filtering.
It checks that a selected app's DNS lookups are filtered and nobody else's
are, that the app resolves normally at once when paused or when the daemon
is killed, and that `abd unhook` leaves nothing of lookup capture. Its fresh host names
come from the wildcard DNS service `nip.io`, which the device must reach.
It exercises the profile owner's always-on VPN, including TCP and UDP/443
recovery after a daemon crash and a tunnel disconnect under lockdown. On
emulators with interface-aware `tcpdump`, it verifies that packets for the
test VPN destination stay off physical interfaces during recovery. Set
`ABD_CHECK_WORK_TILE=1` on an English Android 15 test emulator to also pause
and resume work apps through Quick Settings.
It checks that a new process of an app is given one try of a host the app
pins, and that the host stays pinned when the app refuses it.
Where Vector (the Xposed framework) is installed with ADBlocker's app as a
module, the check also has its app pin a host without the module, enables
the module for the app, and checks that the app's next start has that
host decrypted and no longer pinned. Without them, it says so and skips
that check.
Run both after any change to `abd/src/rules.rs`, `abd/src/trust.rs`,
`abd/src/capture.rs`, `abd/src/dns/` or `module/`.

Run the local HTTPS cosmetic fixture on a rooted emulator with the
module and exclusions installed. The selected app fetches the page using
Android's HTTPS client and verifies the module's certificate. WebView
renders the response with its CSP. The origin runs on the HTTP/3 fixture's
isolated network. The check verifies the scriptlet effect before the first
page script, procedural hiding, and CSP enforcement, then repeats with
cosmetics off.

```sh
./scripts/build-http3-check.sh x86_64
./scripts/check-device.sh --cosmetic
```

The VPN device tests run a separate client APK through Android's VPN. They
cover blocking, pausing, stopping, and restarting the VPN after consent is
revoked. They also cover All apps, exclusion changes, DNS filtering, and
direct system traffic. Use a fresh test emulator with internet access and
a native build from `./scripts/build.sh`. From `android/`, with `ANDROID_SERIAL`
set to that emulator:

```sh
./gradlew :app:assembleDebug :app:assembleDebugAndroidTest
adb install -t app/build/outputs/apk/debug/app-debug.apk
adb install -t app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
adb shell appops set li.mercury.adblocker ACTIVATE_VPN allow
adb shell am instrument -w -e class li.mercury.adblocker.VpnDeviceTest \
    li.mercury.adblocker.test/androidx.test.runner.AndroidJUnitRunner
```

Use `li.mercury.adblocker.VpnDnsDeviceTest` in the same instrumentation
command to check Android's resolver with opportunistic Private DNS. To check
strict Private DNS, set Android's Private DNS provider to `dns.google` and
add `-e strictPrivateDns true` to the instrumentation command. Restore the
original Private DNS settings after the check.

To watch what the daemon does with each connection, start it by hand with
`ABD_VERBOSE=1`. The log then holds what the apps talk to: never ask a user
for such a log.

## Releasing

`module/module.prop` holds the version. A release sets `version` to the tag
(`v0.2.0`) and increases `versionCode`. The same commit adds
`fastlane/metadata/android/en-US/changelogs/<versionCode>.txt`, the
changes in a few sentences, which app stores such as F-Droid show for the
version. The rest of `fastlane/metadata/` describes the app there; its
screenshots are copies of the `*_phone.png` screenshot tests.

Generate a release keystore and keep a private backup. Android uses the
same signing key to accept updates, and the module installs the APK it
carries. For a local build, set `storeFile`, `storePassword`, `keyAlias`,
and `keyPassword` in the ignored `android/keystore.properties`. The store
path is absolute or relative to `android/`.

From a clean checkout of the release tag, run `./scripts/release.sh v0.2.0`
on Linux with the build prerequisites above. The signed
`out/adblocker-<version>.apk.zip` can be flashed directly or renamed to
end in `.apk` for Android installation. Publish it together with
`adblocker-<version>-source.tar.gz`, `adblocker-<version>-SHA256SUMS`
and the R8 mapping. The source archive carries the tagged code, vendored
Rust dependencies, the AndroidX modules that supply native libraries, and
build and dependency details in `release-info/`.

On GitHub, run CI with device checks to test the module and VPN on an
Android 15 emulator. The Release workflow runs these checks for the tagged
commit before it builds and publishes the signed package.

Before the first stable release, install the signed APK and test the root
module on an arm64 device. Check networking after reboot and after
uninstalling the module through the root manager. Record the version and
root manager in the release notes.

The GitHub Release workflow runs this script when a version tag is pushed.
It reads `SIGNING_KEYSTORE_BASE64`, `SIGNING_STORE_PASSWORD`,
`SIGNING_KEY_ALIAS`, and `SIGNING_KEY_PASSWORD` from repository secrets.
Set the first secret to the base64-encoded keystore. Ordinary CI builds
use a debug certificate so contributors can install and test them.
