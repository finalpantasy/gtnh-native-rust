fn gtnh_lv_compressor_eval_screen(
    game: &crate::game::Game<'_>,
    block: &str,
    screen: &mut Option<NativeScreen>,
) {
    use crate::game::gtnh_lv_compressor_eval::adapter;

    if !game.native_on("gtnh_lv_compressor_eval") {
        return;
    }
    if block != adapter::BLOCK || screen.is_some() {
        return;
    }
    *screen = Some(NativeScreen {
        width: 176.0,
        height: 178.0,
        slots: vec![Some([56.0, 17.0]), None, Some([116.0, 35.0])],
        inventory_y: 96.0,
        inventory_x: 8.0,
        own_labels: false,
    });
}