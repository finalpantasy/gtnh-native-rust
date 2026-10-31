fn gtnh_compressor_eval_block_entity_ticks(
    game: &crate::game::Game<'_>,
    block: &str,
    ticks: &mut bool,
) {
    if !game.native_on("gtnh_compressor_eval") {
        return;
    }
    if block == crate::game::gtnh_compressor_eval::adapter::BLOCK {
        *ticks = true;
    }
}
