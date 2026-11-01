fn gtnh_lv_compressor_eval_screen_draw(
    game: &crate::game::Game<'_>,
    block: &str,
    data: &[i32],
    batch: &mut HudBatch,
    origin: [f32; 2],
) {
    if !game.native_on("gtnh_lv_compressor_eval")
        || block != crate::game::gtnh_lv_compressor_eval::adapter::BLOCK
    {
        return;
    }
    if data.len() < 4 {
        game.native_text(
            batch,
            "Awaiting machine state",
            [origin[0] + 8.0, origin[1] + 56.0],
            0xFF404040,
            false,
        );
        return;
    }

    let capacity = data[1].clamp(1, 2048);
    let stored = data[0].clamp(0, capacity);
    let progress = data[2];
    let total = data[3].clamp(1, 300);
    game.native_text(
        batch,
        &format!("Fixture EU: {stored}/{capacity}"),
        [origin[0] + 8.0, origin[1] + 56.0],
        0xFF404040,
        false,
    );
    let progress_label = if progress < 0 {
        format!("Recovery: {} ticks", progress.unsigned_abs())
    } else {
        format!("Work: {progress}/{total} ticks")
    };
    game.native_text(
        batch,
        &progress_label,
        [origin[0] + 8.0, origin[1] + 69.0],
        0xFF404040,
        false,
    );
}