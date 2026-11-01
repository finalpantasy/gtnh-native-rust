fn gtnh_lv_compressor_eval_block_entity_tick(
    game: &mut crate::game::Game<'_>,
    entity: &mut ModBlockEntity,
) {
    use crate::game::gtnh_lv_compressor_eval::adapter;
    use std::sync::atomic::{AtomicBool, Ordering};

    if !game.native_on("gtnh_lv_compressor_eval") {
        return;
    }
    if entity.id != adapter::BLOCK {
        return;
    }

    static TICK_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);
    static TRACE_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);
    match adapter::tick(entity) {
        Ok(_) => match adapter::trace_line(entity) {
            Ok(Some(line)) => eprintln!("{line}"),
            Ok(None) => {}
            Err(error) => {
                if !TRACE_ERROR_REPORTED.swap(true, Ordering::Relaxed) {
                    eprintln!("GTNH_LV_COMPRESSOR_TRACE_FAIL error={error}");
                }
            }
        },
        Err(error) => {
            if !TICK_ERROR_REPORTED.swap(true, Ordering::Relaxed) {
                eprintln!("GTNH_LV_COMPRESSOR_TICK_FAIL error={error}");
            }
        }
    }
}