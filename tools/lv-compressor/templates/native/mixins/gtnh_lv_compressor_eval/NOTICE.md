This first-party evaluation template adds API45 native hook glue and optional
content for gtnh_lv_compressor_eval:lv_compressor. Its host adapter is the
separately maintained source at work/lv-compressor-adapter/adapter.rs. The
native manifest declares the first-party industrial-sim and basic-machine
crates from pack/native-foundation/crates/ as separate dependencies.

Private bundle assembly rewrites exactly three cfg(test) include_bytes paths
in industrial-sim to the bundled fixtures/industrial-v1.json. Production
library source remains unchanged; record the test-only rewrite and fixture
identity with the package identity. Preserve component license notices.

The synthetic furnace-and-iron-ingot recipe exists only to obtain the
evaluation block in a local test world. Fixture EU is operator-supplied test
state and does not represent or connect to a GT electrical network. The policy
covers six declared catalog cases, and its accepted reference oracle is the
measured sand case; this template does not establish complete GTNH recipe,
machine, endgame, multiplayer, or chained-job parity.

Models reference vanilla furnace assets. No game textures, raw game assets, or
reference exports are included.