# Engine integration boundary

Engine repository: https://github.com/finalpantasy/rust-horizons-gtnh
Upstream: https://github.com/horizon-rust/horizon

The first crusher prototype was verified against engine revision `4de90a13d424e08fbbc54507e50420027c2206e2`, native hook API 29. A separate candidate based on nightly revision `763528025d89cf575d7335e2a78d7fc48397d449`, API 40, has compiled and passed isolated adapters, binary-save checks, live registry-alias checks, and the synthetic crusher's real crafting/blocked-output/save/reopen flow. Player-checkpoint recovery and two clean reopens have passed isolated validation and independent review. The validated profile was copied and byte-verified into a separate manual-play profile, and the existing evaluation launcher now selects that candidate through a checked activation manifest. The original profile and backup are preserved. Activation did not launch a game window; physical manual playtesting and extended stability/performance checks remain outstanding. No complete native GTNH pack compatibility is claimed for either revision.

Native mod/content integration should consume a tested engine API. Generic world, rendering, memory and shader features belong in the engine. GTNH recipes, content and behavior belong in this pack. Prototype-only embedded engine hooks are temporary until the pack boundary is verified.

## Use Horizon's native mod system

The integration direction is Horizon's existing [content format](https://github.com/horizon-rust/horizon/blob/main/docs/mods.md) (`.hmod`) and [native Rust mixin bundles](https://github.com/horizon-rust/horizon/blob/main/docs/native-mods.md) (`.hnmod`). Native bundles compile into an instance-specific game and can use versioned native hooks. Reusable missing APIs should be proposed upstream when a reproducible integration need demonstrates the gap. A separate mod loader is not currently planned.

[Horizon Mixins 0.6](https://github.com/horizon-rust/horizon-mixins/blob/main/docs/crates-data-and-named-hooks.md) supports explicit `[[crate]]` bundle libraries, named/versioned hooks and packaged data. This provides a possible boundary for the existing industrial and quest libraries without copying their logic into game hooks. Bundle crates currently require registry dependencies already present in the engine's lockfile; path/git dependencies between bundle crates are not supported.

The candidate launcher's default Mixins revision, `3f99a5c60dfa311cc365524c44ae2fd799bfa1de`, predates that 0.6 capability. Bundles can declare a newer Mixins revision, but the exact tool pin, standalone crate manifests, Windows staging, locked dependencies and composition with another native mod must be tested before claiming a distributable pack. The libraries in this repository are not yet packaged as a working `.hnmod`.

Promote generic foundation experiments into the engine only after worldgen, lighting, physics, persistence, networking and performance checks establish correctness. Early milestones never reduce the complete one-to-one GTNH target.
