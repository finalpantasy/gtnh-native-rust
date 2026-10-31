# Engine integration boundary

Engine repository: https://github.com/finalpantasy/rust-horizons-gtnh
Upstream: https://github.com/horizon-rust/horizon

The first crusher prototype was verified against engine revision `4de90a13d424e08fbbc54507e50420027c2206e2`, native hook API 29. A separate candidate based on nightly revision `763528025d89cf575d7335e2a78d7fc48397d449`, API 40, is being built and tested. No complete native GTNH pack compatibility is claimed for either revision.

Native mod/content integration should consume a tested engine API. Generic world, rendering, memory and shader features belong in the engine. GTNH recipes, content and behavior belong in this pack. Prototype-only embedded engine hooks are temporary until the pack boundary is verified.

Promote generic foundation experiments into the engine only after worldgen, lighting, physics, persistence, networking and performance checks establish correctness. Early milestones never reduce the complete one-to-one GTNH target.
