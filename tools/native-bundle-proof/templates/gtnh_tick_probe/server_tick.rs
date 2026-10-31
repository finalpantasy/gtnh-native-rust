fn gtnh_tick_probe_server_tick(game: &mut crate::game::Game<'_>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static EMITTED: AtomicBool = AtomicBool::new(false);
    if EMITTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = game;
    eprintln!("GTNH_NATIVE_SERVER_TICK_PROBE_OK");
}
