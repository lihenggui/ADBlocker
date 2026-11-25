# Third-party notices

ADBlocker's own code is GPL-3.0-only. The following upstream notices are
preserved with the source.

- The Android convention plugins, navigation, icon loader and screenshot
  helpers include code adapted from [Blocker][blocker]. The affected files
  carry upstream copyright notices, including those of the Android Open
  Source Project and Google LLC. The upstream code and Gradle Wrapper are
  under [Apache-2.0][apache].
- The project history contains a build script adapted from
  [aya-template][aya-template], for an eBPF program that the current
  sources no longer include. Copyright (c) 2021 Alessandro Decina. Its
  [MIT notice][aya-mit] is retained.
- Rust dependencies are specified by `Cargo.lock`. The corresponding source
  archive includes the crates and their original license and notice files in
  `vendor/`. This includes the MPL-2.0 adblock engine and the license files
  for `ring` and its BoringSSL and ISC portions.
- `abd-engine/resources/psl/rules.txt` is the [Public Suffix List][psl]
  as [psl 2.1.259][psl-crate] carries it in `data/rules.txt`, SHA-256
  `47955ddbd523518837878087119ac55d152cb4a570b8fe9b6f79d1da1eb00901`.
  The list is under the [Mozilla Public License 2.0][mpl], whose text is
  in `licenses/MPL-2.0.txt`. Copy it again from the `psl` that Cargo.lock
  takes when that changes: a test of `abd-engine/src/public_suffix.rs`
  fails until then.
- Android dependencies are listed with exact coordinates in
  `android/app/dependencies/`. The corresponding source archive includes
  the license and upstream repository report at
  `release-info/android-licenses.json`. Maven repository definitions and
  the version catalog are in `android/settings.gradle.kts` and
  `android/gradle/libs.versions.toml`. The APK also carries notices
  provided by its Android libraries. [libsu][libsu] is Apache-2.0 and is
  listed in the dependency baseline because its JitPack artifact omits
  license metadata.
- The source archive includes the AndroidX module trees for the two
  Android dependencies that ship native libraries in the APK.
  [Graphics Path 1.1.0][graphics-path] supplies `libandroidx.graphics.path.so`.
  [DataStore Core 1.3.0][datastore-core] supplies
  `libdatastore_shared_counter.so`. Both are Apache-2.0.
- The bundled scriptlet and redirect resources are from [uBlock Origin 1.68.0][ubo],
  commit `b9833670211d83e72c3a855984a3d544945de1e3`.
  Copyright (C) Raymond Hill and other contributors named in the
  source. These files are GPL-3.0-or-later and retain their notices
  under `abd-engine/resources/ublock/`. The per-file SHA-256 hashes
  are in [SHA256SUMS](abd-engine/resources/ublock/SHA256SUMS). The redirect
  names and aliases come from `js/redirect-resources.js` and the bodies
  come from `web_accessible_resources/`. Image and media inputs are kept
  as `.base64` text files, with the original bytes named and hashed in
  the manifest. The generator verifies the hashes, decoding those files
  first. The serialized `scriptlets.json` SHA-256 is
  `45cdae83eebced710833bcb0680f77bd010e0ae54fdc82738a3395eacaf584d7`.
  The serialized `redirects.json` SHA-256 is
  `43ee429468bfc9a1535190e9f9ac3ceca5c28cd9e93e25c5e3ad9d0a0d551f93`.
  Regenerate both with `node abd-engine/resources/generate.mjs`.
  Android builds use the generated files directly.
  The procedural runtime adapts [`contentscript-extra.js`][ubo-procedural]
  at the same commit, upstream SHA-256
  `df8e3afcc6eed42ace647ad83c9d24abfdc391c59fdc0ad9dc66bba0b3acc1df`.
  It retains Raymond Hill's GPL-3.0-or-later notice.
- The AdGuard scriptlet and redirect name mappings use the compatibility
  table and resource aliases from [AdGuard Scriptlets][adguard-scriptlets].
  Copyright AdGuard. GPL-3.0-only.
- The filter catalog references lists that their maintainers serve. The
  license applied to the catalog does not change the licenses of those
  lists.

[blocker]: https://github.com/lihenggui/blocker/tree/ad8bdf369d27061e22db069286e33ad64770a16a
[apache]: licenses/Apache-2.0.txt
[aya-template]: https://github.com/aya-rs/aya-template/tree/09f8aaab53f10beac423d457817e0850bb8c6c08
[aya-mit]: licenses/aya-template-MIT.txt
[libsu]: https://github.com/topjohnwu/libsu/tree/6.0.0
[psl]: https://publicsuffix.org/
[psl-crate]: https://github.com/addr-rs/psl/blob/v2.1.259/data/rules.txt
[mpl]: licenses/MPL-2.0.txt
[graphics-path]: https://android.googlesource.com/platform/frameworks/support/+/1c44f0be720ff16cd30357ad2375239bf97a1da8/graphics/graphics-path/
[datastore-core]: https://android.googlesource.com/platform/frameworks/support/+/de4cda56f812357e582b3e1aa32bc737636614a2/datastore/datastore-core/
[ubo]: https://github.com/gorhill/uBlock/tree/b9833670211d83e72c3a855984a3d544945de1e3
[ubo-procedural]: https://github.com/gorhill/uBlock/blob/b9833670211d83e72c3a855984a3d544945de1e3/src/js/contentscript-extra.js
[adguard-scriptlets]: https://github.com/AdguardTeam/Scriptlets/tree/420ffa66c6e713ffa95c80e4bfa356b1a796c3a3
