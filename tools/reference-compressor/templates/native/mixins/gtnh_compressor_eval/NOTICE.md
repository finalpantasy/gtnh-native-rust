This first-party evaluation bundle combines the source bridge and adapter in
`support/` with the separately declared `industrial-sim` library crate.
The library's source files are copied unchanged from
`native-rust/pack/native-foundation/crates/industrial-sim/src/`; only the
bundle Cargo package name is qualified in this copy. The bridge is adapted
from `native-rust/pack/native-foundation/bridges/horizon_machine.rs` to use
the exact fixture bytes embedded by the source assembler. No Mojang textures
are included; the optional content package references vanilla furnace models.
