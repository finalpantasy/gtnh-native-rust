`crates/industrial-sim/src/` and `crates/quest-core/src/` are byte-for-byte copies of the source
libraries in `native-rust/pack/native-foundation` at preparation time. The test-only Cargo package
names are qualified as `gtnh-foundation-proof-*` for the bundle crate namespace. Version, edition,
Rust version, license, direct dependencies, and the `unsafe_code` lint are made literal from each
source manifest and its workspace values; no library Rust source is changed.

`crates/quest-core/src/logic.rs` adapts the Boolean gate semantics from `GTNewHorizons/BetterQuesting`,
version `3.8.92-GTNH`, commit `17d5f92f74144594c2890babc051c52032b7ec40`,
`src/main/java/betterquesting/api/enums/EnumLogic.java` (MIT; copyright 2019 Funwayguy). The MIT
license is preserved in `licenses/LICENSE-BetterQuesting-MIT.txt`. The copied source retains its
original relative attribution comment (`../../../licenses/BetterQuesting-MIT.txt`); the package
filename uses the `LICENSE-*` prefix required by Horizon's `.hnmod` file allowlist. Root-quest and
optional-task rules were inspected in the same revision's MIT `QuestInstance.java`; synthetic
fixtures do not include pack prose, assets, or rewards.

`crates/quest-core/src/item_match.rs` adapts the same revision's MIT
`src/main/java/betterquesting/api/utils/ItemComparison.java` and numeric semantics from
`NBTConverter.getNumber`. It deliberately preserves BetterQuesting's asymmetric compound and legacy
list/array behavior; it adds finite value, codec, and work bounds and does not claim generic Minecraft
NBT equality, binary parsing, or ore-dictionary/task parity. The preserved MIT license and copyright
above apply. Development oracle API models are first-party test scaffolding, not Minecraft
implementations.

`crates/quest-core/src/retrieval.rs` adapts single-player counting, reset, and allocation rules
inspected in the same MIT revision's `bq_standard/tasks/TaskRetrieval.java` Detector and donation
path. It adds bounded staged publication and codec identity and explicitly excludes party, group, ore,
auto-consume, and whole-task completion callbacks. The same preserved MIT license and copyright
apply; no pack prose or rewards are included.
