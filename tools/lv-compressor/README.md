# GTNH LV Compressor Source Package

This folder contains the portable, source-only packager for the pinned LV
compressor evaluation bundle. The packager uses Python 3.11 or newer and the
standard library. It reads only the SHA-256-locked files under
`gtnh-runtime/` and `native-foundation/`, plus the allowlisted templates and
support sources in this folder.

The generated `.hnmod` archive keeps `industrial-sim` and `basic-machine` as
separate crates. Its standalone Cargo manifests preserve both public package
names and the `industrial-sim` dependency alias. Three test-only
`include_bytes!` paths are rewritten to use the bundled fixture; the runtime
adapter gets one `native_data!` fixture transform, and its exact child test
module declaration is removed from the packaged runtime source.

Both crates occupy top-level bundle folders; the industrial test fixture is
under `industrial-sim/data/`. Horizon's Rust bundle reader accepts this layout.
The earlier archive put the fixture in a nested `crates/.../fixtures/` folder,
which the normal launcher rejected. The correction changes packaging paths
and three test-only includes; the original production sources are preserved.

The support adapter is an assembler input, not a standalone Cargo crate. The
original private adapter test harness is not included because it refers to a
repository-specific fixture path. The preserved `NOTICE.md` retains its
historical relative provenance path `work/lv-compressor-adapter/adapter.rs`;
that source is available here as `support/adapter.rs`. The approved MIT
license and notice files are preserved byte-for-byte. The archive contains no
game binary, raw reference export, or copied game texture.

## Reproduce the archives

From the public repository root, choose a fresh `build-*` output child of this
folder:

```powershell
python tools/lv-compressor/package_compressor.py `
  --output-dir tools/lv-compressor/build-reproduction-1
```

Give each invocation a unique fresh `build-*` path and own that path exclusively
until the process exits; concurrent writers targeting the same path are
unsupported. The packager checks the exact `gtnh-runtime/`,
`native-foundation/`, support-file, and 19-template allowlists; validates their
pinned hashes and manifest identities; verifies deterministic archive
metadata and members; and checks both expected archive hashes. It stages files
in a sibling directory and renames that directory into place after a
preflight check. A populated destination found at preflight is preserved, but
the preflight and rename do not provide a concurrency-safe no-clobber
guarantee: on POSIX, a concurrently created empty destination may be replaced.
The output contains the two archives, their unpacked source members, and
`lv-compressor-package-receipt.json`.

Windows with Python 3.14.6 was tested. Python 3.11 and Unix were not executed.

The repository-root `.gitattributes` preserves the byte-locked inputs during
checkout. Keep those rules in place; the packager intentionally rejects any
line-ending conversion or other byte change.

The expected SHA-256 values are:

- `.hnmod`: `d9cfe6d35bc82c7c2353c2b0018affc8d8cd8a0def0f79bcf696c8d48ab318c5`
- `.hmod`: `ab94a30985055ad725e43753997d415da0270cc7b99e4680a9bf3aa65812a9b6`

Run the source-only package checks from the repository root with:

```powershell
python -m unittest discover -s tools/lv-compressor -p test_package.py -v
```

## Runtime and verification scope

The package pins Horizon revision
`681124a485b82a27ac4e769756a2f3a2356e6827` and hook API 45, with native SDK
revision `3ac63df26d332bc344ba645d5d23497b7d4ad985`. The SDK same-bundle library
dependency change is contributed in [draft upstream PR 18](https://github.com/horizon-rust/horizon-mixins/pull/18);
this does not claim the change was merged. Normal Windows launcher installation
and four fresh installed-client save/reopen sessions passed with launcher
commit `5515c06762a0634f037e643bd021dd0f7f1bb800`, contributed as
[draft Horizon PR 5756](https://github.com/horizon-rust/horizon/pull/5756).
Unmodified stock-launcher installation remains unverified.

The operator-only, level-4 fixture commands are:

```text
/gtnhlvcompressor charge <x> <y> <z> <eu>
/gtnhlvcompressor trace <x> <y> <z> <label>
```

Charge accepts 1–2048 fixture EU and does not connect an electrical network.
Trace labels are `nominal`, `shortage`, and `blocked_output`. These commands
exist for controlled evaluation; they do not establish production power
delivery or full GTNH behavior.

The accepted bounded run independently replayed 1,204 reference rows and a
four-phase native save/reopen proof using the earlier native archive
`c63fa988f60e94920b09ec5f71ebc3ed74e51117fe89f9f984ea22f726235fcc`.
The corrected archive now passes a separate four-session save/reopen proof
using the normally installed executable and normal content-mod discovery.
The 1,204-row comparison was not rerun against that executable. The
content archive is unchanged. The earlier replay covers sand-to-
sandstone processing. Whole-pack parity, network power, original chained-job
scheduling, machine removal behavior, and stock-launcher installation remain
unverified. The [LV evaluation report](../../docs/lv-compressor.md) records the
evidence and limits; the [recovery screenshot](../../docs/images/lv-compressor-recovery.png)
is an actual cropped game capture.
