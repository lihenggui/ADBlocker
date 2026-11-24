# AGENTS.md: Android app

The app is how most people use ADBlocker: it chooses the apps to filter,
the lists and the rules, shows what was blocked, and installs the module.
It follows the module structure of Now in Android, as Blocker and Big
Brother do: convention plugins in `build-logic/`, versions in
`gradle/libs.versions.toml`, Hilt, Compose with Material 3 Expressive, and
Navigation 3. The APK is also the flashable module (`FlashableModuleConventionPlugin`):
the module's scripts and a daemon per architecture are at the root of the
archive, where root managers look for them.

## Modules

- `app/`: the application, the navigation bar or rail and the list-detail
  layout (`ui/ADBlockerApp.kt`), its pane and navigation composables
  (`ui/ShellNavigation.kt`), the launcher and splash icons, the Quick
  Settings tile (`tile/`), and the receiver of the boot broadcast (`boot/`).
- `benchmarks/`: the startup Baseline Profile and cold-start Macrobenchmarks.
  Generate the release profile with `./gradlew :app:generateReleaseBaselineProfile`
  on an API 33+ test device. Run `./gradlew :benchmarks:connectedBenchmarkReleaseAndroidTest`
  on a physical device to compare startup with and without precompilation.
- `hook/`: the LSPosed module (libxposed API 102, `META-INF/xposed/`, no
  fixed scope), loaded only into the apps the user enables it for in
  LSPosed. It lets their own certificate checks accept a chain that ends
  in the root module's local root (`PinPolicy`), and runs each check as
  the app wrote it first (`relaxed`). Its tests make certificates as
  `abd-engine/src/ca.rs` does.
- `lint/`: host-side Kotlin source structure detectors and their tests.
- `vpn/`: the VPN method's `VpnService` (`AdblockerVpnService`), which
  builds the VPN for the chosen apps and hands it to the engine
  (`VpnSession`), its notifications, and `VpnController` for the screens.
- `core/model`: data types, the filter method (root module or VPN) and
  what each can do (`Capabilities`). `core/data`: repositories;
  `transport/` reaches `abd` through the root shell (`RootTransport`) or
  the engine in this process (`EngineTransport`), as the method in use
  says; `root/RootShell.kt` is the only place the app runs root commands,
  and `protocol/AbdProtocol.kt` the only place it reads what `abd` prints.
- `core/engine`: the engine as the native library `libabd_vpn.so`, for
  filtering without root. Its contract is `../docs/engine-jni.md`; a change
  to it changes the library in the same commit. `core/datastore`: settings.
  `core/ui`, `core/designsystem`: shared UI, the theme, the icons and the
  status mark. `core/navigation`: navigator and scene strategies.
  `core/common`: dispatchers, DI qualifiers, logging and cancellation-safe
  failure handling for view models.
- `core/testing`: `MainDispatcherRule` for unit tests.
  `core/screenshot-testing`: Roborazzi helpers, the demo apps and data.
- `feature/<name>/api` (navigation keys) and `feature/<name>/impl` (screens,
  view models) for overview, apps, appdetail, filters (lists, rules, trusted
  sites, the rule editor), activity (recorded requests), settings and https
  (installing the certificate for the VPN method). Overview keeps its
  status, statistics and notices in dedicated files. Filters has separate
  search and tab composables. Settings keeps its rows and sheets beside
  the screen host, and the app detail screen has separate troubleshooting
  composables.
  `feature/onboarding/impl` hosts setup before the rest of the app and has
  no navigation key. The individual setup pages live in its `steps/` package.
- `sync/work`: the background list update (WorkManager), which runs
  `abd update`; the daemon has no timer of its own.

## Commands

Run from `android/`:

```sh
./gradlew spotlessApply                     # ktlint and license headers
./gradlew test                              # unit and screenshot tests
./gradlew lintStructure -PlintStructureOnly  # production Kotlin limits
./gradlew recordRoborazziDebug              # re-record screenshots after a UI change
./gradlew :app:dependencyGuard :app:licenseeAndroidRelease :app:lintRelease
./gradlew :app:checkReleaseBadging          # run separately from updateReleaseBadging
```

Assembling `:app` needs the daemons in `../out/` (`../scripts/build.sh`).

## Talking to the daemon

- The app asks the daemon only through `abd ... --json`: run as root with
  the root module, or by the engine in this process with the VPN method,
  which answers the same. Each output is in `../testdata/protocol/`, which
  `AbdProtocolTest` and the daemon's own tests read. A change to an output
  changes its case and both sides in the same commit.
- An exit code of 2 means the command or arguments were rejected. 126 and
  127 mean the module is missing. Neither error is retried in a loop.
- `abd watch --json` and `abd update --json` are streams, each in its own
  `su --mount-master` process (`RootShell.stream`), ended by closing it.
  Nothing else holds a process open. One list update runs at a time in the
  process (`ListsRepository`).
- Every root command runs under `timeout` (60 s, longer for installing
  and removing the module), in the global mount namespace; exit 124 is
  `AbdException.TimedOut`.
- The module is flashed in a root manager or installed by the app with
  root alone, and either can change while the app runs. A command that
  names the module's files begins with `ModulePaths.LOCATE` and names them
  through `MODULE_DIR`; `ModulePaths.ABD` does so for `abd`. The app
  installs with the `direct.sh` in its own APK (`ModuleRepository`).
- A root command can show the root manager's notice or prompt. Nothing
  runs one unasked except `AutoStart`, once after a boot (`BootReceiver`
  in `app/`), and only in root mode with a module the app installed, as
  the settings remember it (`UserData.moduleInstalledByApp`): root is not
  asked whose the module is in order to find out. It starts a stopped
  daemon where no root manager ran the module's launcher, never turns a
  module on again, and is not tried a second time.
- Screens ask `Capabilities` what the method in use can do, never the
  method itself. Without the engine in the build, the VPN method's commands
  throw `AbdException.EngineMissing`.
- The VPN routes the chosen apps or the attributable personal apps in
  All apps, depending on scope. Android applies the allowlist by UID.
  Rebuild after package changes and exclusions. Never establish an empty
  allowlist, which Android interprets as a VPN for everyone. Pausing never builds it
  again. Whatever stops the engine stops the service, and the apps are on
  the network unfiltered.

## Code

- Root commands go through `RootShell`. Quote every value that comes from
  an app or the user with `shellQuoted()`; text such as rules goes on
  standard input, never on the command line. The label is what gets
  logged; never put a host, a URL or a package name in it.
- Features use repositories and shared models. Keep transport, storage and
  platform I/O below UI and keep Gradle module dependencies acyclic.
- Use `VpnAppRepository` for VPN selection and status, `RuleDraftStore` for
  drafts, `DocumentWriter` for writing documents and `AppPlatformInfo` for labels and permissions.
- Use `ReadState` to represent an unavailable read separately from an
  available empty result. Surface partial failures with the data still available.
- Repositories expose suspend functions or flows and throw `AbdException`;
  view models map it to `LoadFailure`. View models expose a `StateFlow` of
  UI state and keep pending snackbar outcomes in `MessageQueue`. A view model
  loads in one job at a time. Passive reads follow the screen's lifecycle.
  Wrap suspend calls with `suspendRunCatching`,
  never `runCatching`, which swallows cancellation.
- Expose derived state with `stateIn(viewModelScope, WhileSubscribed(5_000), …)`.
  Keep work out of `init` and keep Android `Context` out of ViewModels.
- Keep ViewModels at screen entry points and pass state and callbacks down.
- A change the daemon reports through `DataChanges` is signalled in a
  `finally`, so that a partial failure is read back too.
- Screens have a public entry point that takes the view model and an
  internal, stateless overload that takes the UI state; tests use the latter.
- Layouts work from compact phones to tablets and unfolded foldables: a
  navigation bar, a collapsed rail, or an expanded one from the large
  breakpoint; a list beside its detail from the expanded breakpoint; pages
  in one pane no wider than `MaxContentWidth`. Check a new screen at more
  than one width.

## Material 3 Expressive

The design follows the specification at m3.material.io, not memory of it.
What the app relies on:

- Motion uses `MotionScheme.expressive()` springs for components: spatial
  springs for position, size, rotation and shape, effects springs for color
  and opacity. Screen transitions still use easing and duration, as M3's
  transition patterns do, and live in `core/designsystem/.../motion/Motion.kt`:
  forward and back slide 96dp with a fade (350 ms in, 200 ms out), every
  move between top-level destinations fades out then in (the `Navigator`
  knows which moves those are), tabs slide together without fading.
- Lists are segmented (`SegmentedGroup`, `SegmentedItem`): items on
  `surfaceContainer` over `surface`. Only a row chosen among others takes the
  selected color (`showsSelection`); a row with a switch keeps its own.
- One progress indicator for many items (the wavy one for list updates); a
  loading indicator for a wait under five seconds.
- Each pending snackbar outcome has a stable ID. The screen acknowledges
  that ID after handing the outcome to the shared host. A snackbar with an
  action stays until used or dismissed, and closes with the screen whose
  action it offers. Only the front pane shows snackbars (`AppSnackbarHost`).
  A snackbar without an action outlives the screen once handed to the host.
- Type comes from `Type.kt`, whose emphasized styles carry M3 Expressive's
  weights: Compose's default `Typography()` makes them equal to the regular
  ones.
- Shape is not a meaning: the status mark morphs only between states.

## User interface text

- Every string goes in `values/strings.xml` and `values-zh/strings.xml`.
- Copy is short and factual: what happens, not how the daemon works. In
  Chinese, do not address the user with "你的".

## Privacy

- The network permission is for the VPN method: the engine in the app's
  process downloads the lists and connects onward for the chosen apps.
  With the root method the module does both, as root, and the app does not
  connect. Nothing in the app connects anywhere else.
- The activity recording service receives requests only after the user
  starts recording. It keeps at most `MAX_ACTIVITY_EVENTS` in memory,
  including after stopping, and never writes or logs them. A request opens
  by its id in the recording; hosts and URLs never go into navigation keys,
  which are saved with the back stack.
- The rule editor saves unsaved text in the app's private no-backup directory.
  Its saved state holds only the draft's id. Saving or discarding clears the
  draft, and the editor removes old abandoned drafts when opened.
- Exception messages can quote what abd printed: the crash log keeps class
  names and stack frames only, and `BadOutput` names the command, not the
  output. Hosts that pin their certificate are read from the daemon when
  shown, and never cached.
- The persisted log (`ReleaseTree`) must not contain hosts, URLs or package
  names. Nothing is backed up (`data_extraction_rules.xml`).

## Tests

- Unit tests run on the JVM with Android calls returning defaults.
- Name new tests `givenCondition_whenAction_thenResult`. Use
  `kotlinx.coroutines.test.runTest` for coroutine tests. Keep waits for
  Android services and other external callbacks on a real dispatcher.
- Screenshot tests compare with checked-in images, so a UI change fails
  `./gradlew test` until the images are recorded again. Sample data uses
  made-up apps under `com.example.*` only (`DemoApp`, `DemoData`).
- Use `DemoClockRule` to pin the screenshot clock. Start collecting
  subscription-driven state in tests before asserting on it.

## Dependencies and baselines

- The release APK's libraries (`app/dependencies/`), its badging
  (`app/release-badging.txt`) and the license allow-list
  (`build-logic/.../Licensee.kt`, GPL-3.0-compatible licenses only) are
  checked in CI. Update a baseline only for an intended change, in the same
  pull request.
