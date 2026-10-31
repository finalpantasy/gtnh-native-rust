# Native Bundle Proof Packaging

The packager produces two bounded Horizon proof bundles: `gtnh_foundation_proof` and
`gtnh_tick_probe`. They pin Horizon `681124a485b82a27ac4e769756a2f3a2356e6827` and Mixins
`4342a0bfbe71d4d14185528d64e496d6022f16a1`, and include selected Rust libraries plus a `server_tick`
hook. The hook contract is version 1; it was introduced at native hook API 4, while the pinned
Horizon revision exposes API 45. These bundles do not contain the complete GTNH modpack.

## Reproduce the packages

Requires Python 3.11 or newer. From the pack checkout, choose a new output directory outside it:

```text
python tools/native-bundle-proof/package_bundles.py --pack-root . --output-dir ../gtnh-native-proof-bundles
```

The output directory must not exist; its parent must exist. The output includes both `.hnmod`
archives and `bundle-pack-receipt.json`. The receipt records relative source paths, archive hashes,
and per-entry hashes without machine-specific paths.

`bundle-lock.json` pins the archive hashes, source-file allowlists, manifests, licenses, templates,
and Horizon/Mixins revisions. The tool checks workspace metadata, normalizes text inputs to LF, and
writes the probe bundle's `LICENSE` as canonical CRLF. It rejects source drift and reproduces the
locked archive bytes. It does not run Cargo, compile Rust, launch Horizon, or start the game.

## Run the native build proof

For a disposable instance, place both `.hnmod` files in its normal `horizon-native-mods` archive
folder and enable both IDs in `horizon-mods.txt`:

```text
mod.gtnh_foundation_proof:true
mod.gtnh_tick_probe:true
```

Use a launcher built from the pinned Horizon revision, install the signed proof base and the
launcher's Rust toolchain, and prepare Minecraft 26.3 resources from your own game files. The
[upstream native-mod guide](https://github.com/horizon-rust/horizon/blob/681124a485b82a27ac4e769756a2f3a2356e6827/docs/native-mods.md)
describes launcher setup. With the disposable instance and both archives configured, run:

```text
horizon-launcher native-build --instance <disposable-instance> --version nightly-2026-10-31-d91cab6 --profile fast --trust-native-code
```

The package receipt verifies source and archive contents. Runtime verification separately requires
both complete log lines, exactly once:

```text
GTNH_NATIVE_FOUNDATION_LIBS_OK id=gtnh:probe all_complete=true
GTNH_NATIVE_SERVER_TICK_PROBE_OK
```

## Verified result

On October 31, 2026, the real Windows launcher built and selected
`native-681124a-6513d2ec4d62` using its portable Rust 1.99.1 GNU toolchain and optimized `fast`
profile. All 437 registry package identities and all 15 packaged Rust library source files stayed
unchanged. The installed executable matched the compiler output.

The first fresh-world observation emitted both markers once and exited cleanly, but its strict
integrity check correctly failed when Horizon regenerated `horizon-sprites.cache`. That cache uses
texture paths, sizes and timestamps as its key. The only changed file was the cache; all 1,301 decoded
sprite payloads were byte-identical to the original cache. Initialization was recorded separately.

A second fresh-world run passed the unchanged strict observer: both exact markers once, a live
player in the requested world, exit code 0, no forced shutdown, and every source/resource file and
the executable unchanged, including the initialized cache. The game observation took 20.353 seconds.
The actual launcher installation, cache initialization and final runtime evidence received independent
review. [Verification metadata](native-bundle-proof-verification.json) records pins, hashes and limits.

This verifies the two-library boundary and coexistence of two native hooks. It does not establish
compressor gameplay, a complete GTNH port, performance benchmarks or extended stability. The existing
API-40 manual candidate and player profiles were not used by this proof.
