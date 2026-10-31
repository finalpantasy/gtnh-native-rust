fn gtnh_compressor_eval_block_entity_tick(
    game: &mut crate::game::Game<'_>,
    entity: &mut ModBlockEntity,
) {
    use crate::game::gtnh_compressor_eval::adapter;
    use std::sync::atomic::{AtomicBool, Ordering};

    if !game.native_on("gtnh_compressor_eval") {
        return;
    }
    if entity.id != adapter::BLOCK {
        return;
    }
    static ERROR_REPORTED: AtomicBool = AtomicBool::new(false);
    if let Err(error) = adapter::tick(entity)
        && !ERROR_REPORTED.swap(true, Ordering::Relaxed)
    {
        eprintln!("GTNH_COMPRESSOR_TICK_FAIL error={error}");
    }
}
