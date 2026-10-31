fn gtnh_compressor_eval_screen_draw(
    game: &crate::game::Game<'_>,
    block: &str,
    data: &[i32],
    batch: &mut HudBatch,
    origin: [f32; 2],
) {
    if !game.native_on("gtnh_compressor_eval")
        || block != crate::game::gtnh_compressor_eval::adapter::BLOCK
    {
        return;
    }
    let stored = data.first().copied().unwrap_or(0).clamp(0, 4096);
    let capacity = data.get(1).copied().unwrap_or(4096).clamp(1, 4096);
    let progress = data.get(2).copied().unwrap_or(0).clamp(0, 300);
    let total = data.get(3).copied().unwrap_or(300).clamp(1, 300);
    game.native_text(
        batch,
        &format!("Test power: {stored}/{capacity} EU"),
        [origin[0] + 8.0, origin[1] + 56.0],
        0xFF404040,
        false,
    );
    game.native_text(
        batch,
        &format!("Work: {progress}/{total} ticks"),
        [origin[0] + 8.0, origin[1] + 69.0],
        0xFF404040,
        false,
    );
}
