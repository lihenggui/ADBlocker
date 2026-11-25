## Summary

<!-- What changed, and why? -->

## Verification

Mark the checks that passed. Note any skipped or failed checks and why.
See [CONTRIBUTING.md](https://github.com/lihenggui/ADBlocker/blob/main/CONTRIBUTING.md#checks).

### Host checks

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test` and `cargo deny check`
- [ ] Shell syntax and the module boot, uninstall and direct install checks
- [ ] `sh scripts/check-engine-jni.sh`
- [ ] `actionlint`, if installed
- [ ] `./scripts/build.sh --locked`
- [ ] Gradle `spotlessCheck`, `:build-logic:convention:spotlessCheck` and `test`
- [ ] Gradle `lintStructure -PlintStructureOnly`
- [ ] Gradle `:app:dependencyGuard`, `:app:checkReleaseBadging`, `:app:licenseeAndroidRelease` and `:app:lintRelease`

### Device or emulator

- [ ] Ran the applicable [device checks](https://github.com/lihenggui/ADBlocker/blob/main/CONTRIBUTING.md#on-a-device) for changes to kernel, routing, trust, DNS, module, VPN or Android behavior. Record the device or emulator, checks and results below.

### Documentation and privacy

- [ ] Updated the affected documentation, `SECURITY.md` and protocol fixtures in `testdata/protocol/` where applicable.
- [ ] Default logs omit users' host names, URLs and package names. Any saved values stay in private storage.
