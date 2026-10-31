fn gtnh_foundation_proof_server_tick(game: &mut crate::game::Game<'_>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static EMITTED: AtomicBool = AtomicBool::new(false);
    if EMITTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = game;

    let id = match gtnh_foundation_proof_industrial_sim::content::Id::new("gtnh:probe") {
        Ok(id) => id,
        Err(_) => {
            eprintln!("GTNH_NATIVE_FOUNDATION_LIBS_FAIL operation=industrial_id");
            return;
        }
    };
    let all_complete = match gtnh_foundation_proof_quest_core::logic::Logic::AND.evaluate(2, 2) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("GTNH_NATIVE_FOUNDATION_LIBS_FAIL operation=quest_and");
            return;
        }
    };
    if id.as_str() != "gtnh:probe" || !all_complete {
        eprintln!("GTNH_NATIVE_FOUNDATION_LIBS_FAIL operation=expected_values");
        return;
    }
    eprintln!(
        "GTNH_NATIVE_FOUNDATION_LIBS_OK id={} all_complete={}",
        id.as_str(),
        all_complete
    );
}
