# GTNH basic machine policy slice

This standalone crate implements the fixture-backed low-voltage compressor slice using the shared `industrial-sim` catalog and inventory types. It is opt-in and accepts a selected recipe only when it has one plain-item input entry (using that recipe's declared positive quantity), one plain-item output of one item, a 300-tick duration, and a 2-EU/t rate.

`Compressor::new` takes the machine kind and input/output `Limits`. At idle, `tick` selects a recipe from the supplied `Catalog` using the machine kind and current inputs. A selected declaration outside the supported shape returns an error without changing state. `fixture_charge` is a test/evaluation energy action; it does not model a network, voltage packet, or startup schedule.

The policy uses a 2,048-EU store, a 256-EU intake threshold, an unpowered intake tick, a signed 100-tick failed-drain cooldown, and checked received/spent/wasted-work ledgers. Inputs and outputs are `industrial-sim::Inventory` values. Input removal, output delivery, energy changes, ticks, and JSON decode are validated before commit. Output insertion while a batch is active is rejected; output removal remains available for blocked-port recovery.

JSON saves carry schema and policy versions, port contents and limits, fixture energy, ledgers, completion count, and the exact active `Recipe` with held inputs and pending output state. Active saves are checked against the supplied catalog's exact recipe definition and referenced item kinds. `Catalog` has no whole-catalog digest, so idle saves are not cryptographically bound to every unrelated catalog entry; a content change to an active recipe is rejected. There is no migration from the earlier prototype's save format.

The integration test replays every row of `fixtures/compressor-lv-v1.json`, applies its after-sample actions, and round-trips JSON after every sample. This verifies the Rust model against that sanitized measured trace only. It does not establish Java save/reopen parity, GUI behavior, network behavior, generic GregTech machine parity, or full-pack coverage.

`verification.json` records this earlier policy-only check. The later [native integration report](../docs/lv-compressor.md) separately documents the actual Horizon processing and four-session save/reopen proof, with its own evidence and limits. The [source packaging tool](../tools/lv-compressor/README.md) integrates this policy through Horizon's native mod boundary.
