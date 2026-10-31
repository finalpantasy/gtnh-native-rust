# GTNH Native Rust

A complete, one-to-one native Rust port of GregTech: New Horizons is the project goal: every mod, all content and behavior, progression through the endgame, and cross-mod interactions. This is early development. The complete pack is not implemented or loadable yet.

The separately playable Rust Minecraft base lives in the [Rust Horizons GTNH engine fork](https://github.com/finalpantasy/rust-horizons-gtnh), based on [Horizon](https://github.com/horizon-rust/horizon). This repository owns the native GTNH pack and its compatibility tests. Pack-specific gameplay must remain optional for vanilla play.

![Actual native Rust foundation laboratory](docs/images/native-foundation-lab.png)

Actual development-lab capture: synthetic processing, quest submission/reward accounting and cubic inspection. It demonstrates the current foundation libraries, not completed GTNH content or final game presentation.

## Current code

`native-foundation/` is a runnable independent experiment containing industrial simulation, item/fluid transfers, quest primitives, transactional storage, cubic streaming, budgeted GPU resources and a native inspection viewer. The world/rendering modules are integration experiments pending adoption into the engine; they are not permanent GTNH-only engine requirements. `bridges/horizon_machine.rs` is an optional, separately verified Horizon adapter experiment, not a registered pack mod.

The current recipes/tasks in the lab are synthetic. Six separately labeled compressor parameter fixtures record source-declared facts, not complete registered-recipe parity. `reference/gtnh-2.9.0/mods.json` retains all 243 pinned manifest entries with explicit unfinished status. No upstream mod is silently dropped.

## Run the library checks

The commands below require Rust 1.95 or newer because they include the desktop viewer. The core libraries separately declare Rust 1.90.

```sh
cargo test --manifest-path native-foundation/Cargo.toml --workspace --all-features --locked
cargo run --manifest-path native-foundation/Cargo.toml -p foundation-viewer --features desktop -- --new
```

The viewer is a separate technical lab, not the first-person Horizon client. Its current graphics backends are Vulkan and DX12; Metal is not configured. Test fixtures create private temporary worlds. On Windows, filesystem and graphics permissions must allow those operations.

## Repository boundaries and roadmap

- Engine fork: standalone vanilla play, cubic chunk management, terrain/object LOD, async memory-budgeted assets, multithreading, reliable saves, native mod SDK, native shaders and multiplayer/server infrastructure.
- This pack: every native mod port, recipes/materials/machines/multiblocks, energy and logistics, world content, magic/research, quests/rewards, interfaces and complete progression; plus reference-backed behavior and interaction checks.
- Integration: pin tested engine revisions and native API versions. Keep the current crusher prototype working while the compressor and wider pack are integrated through the native mod boundary.
- Compatibility: track behavioral discrepancies and save/world import, shader interchange and networking investigation explicitly. Engine improvements must preserve GTNH semantics.

The original configured GTNH pack is a private Java comparison oracle. The shipping engine and mod implementations remain native Rust. Initial reference startup observations cover 11,343 item registry entries, 4,258 blocks, 64,994 crafting recipes and 185,170 GregTech recipe entries across 172 maps at the client-menu stage. These are not full item-variant counts or completed ports; server/world startup and conditional content still need comparison.

## Licensing

First-party code is MIT; preserved third-party notices are in `native-foundation/THIRD_PARTY_NOTICES.md`. Each future mod port and asset requires its own license/permission review. Original Minecraft/GTNH binaries, assets, pack archives, private worlds, credentials and raw runtime exports are not included. Download access does not grant redistribution rights.

This is an independent project, not an official GTNH, Horizon or Mojang release.
