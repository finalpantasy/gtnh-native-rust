fn gtnh_lv_compressor_eval_block_entity(
    game: &mut crate::game::Game<'_>,
    block: &str,
    entity: &mut Option<(String, horizon_world::nbt::Compound)>,
) {
    use crate::game::gtnh_lv_compressor_eval::adapter;
    use std::sync::atomic::{AtomicBool, Ordering};

    if !game.native_on("gtnh_lv_compressor_eval") {
        return;
    }
    if block != adapter::BLOCK || entity.is_some() {
        return;
    }
    static ERROR_REPORTED: AtomicBool = AtomicBool::new(false);
    match adapter::new_entity() {
        Ok(created) => *entity = Some((adapter::BLOCK.to_owned(), created.nbt)),
        Err(error) => {
            if !ERROR_REPORTED.swap(true, Ordering::Relaxed) {
                eprintln!("GTNH_LV_COMPRESSOR_CREATE_FAIL error={error}");
            }
        }
    }
}