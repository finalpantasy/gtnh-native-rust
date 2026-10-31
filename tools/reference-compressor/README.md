# GTNH Reference Compressor Source Package

This directory contains a portable, source-only packager for the optional
`gtnh_compressor_eval` prototype. It produces a native source archive (`.hnmod`)
and a small content archive (`.hmod`) from the allowlisted
`pack/native-foundation` Rust library and fixture, plus the templates and
support sources stored here. It uses Python 3.11 or newer and only the Python
standard library.

## Scope

The prototype covers six declared 300-tick, 2-EU/t compressor cases and an
operator-level test-charge command. It does not claim complete GTNH recipe,
machine, progression, energy-transfer, or removal parity. A known boundary is
that generic block removal spills host item slots and discards the active
recipe state. Pinned GTNH source indicates that ordinary breaking also loses
the consumed batch; an actual reference removal comparison is still pending.

The content archive references vanilla furnace models and includes no
textures. The MIT license and source notice are preserved. The private adapter
test harness, game files, compiled libraries, and machine-specific paths are
not packaging inputs.

## Reproduce the source archives

From the pack checkout, choose a fresh output directory outside the checkout:

```powershell
python tools/reference-compressor/package_compressor.py `
  --pack-root . `
  --output-dir ../gtnh-reference-compressor-bundles
```

Use a different unused output directory for each run. The packager checks the
strict source and template allowlists, their SHA-256 hashes, the six-case
fixture, archive member bytes and metadata, and both pinned reference archive
hashes. It writes the two archives and `compressor-package-receipt.json` to
the selected directory. No file is written into `pack/` or this source tree.

The locked v3 archive hashes are:

- `.hnmod`: `30775d92e6b50683105e124376fb74a27d1e83fcae4f84ed4e6292c32fd5b2ff`
- `.hmod`: `4078fa41ed3b956bc8a93369245ae3befd4fec53360eedec29d3cb2c864bad4a`

## Verification status

Reproducing the source archives does not run Cargo, compile Rust, stage the
launcher, launch a game, or verify in-world behavior. The packager therefore
reports `RUNTIME_NOT_VERIFIED` for its own operation. Separate actual-game
results and their limits are recorded in the [compressor verification guide](../../docs/reference-compressor.md).
