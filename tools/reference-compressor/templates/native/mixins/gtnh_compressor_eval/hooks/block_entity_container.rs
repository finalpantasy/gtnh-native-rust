fn gtnh_compressor_eval_block_entity_container(
    game: &crate::game::Game<'_>,
    block: &str,
    container: &mut Option<ModContainer>,
) {
    use crate::game::gtnh_compressor_eval::adapter;

    if !game.native_on("gtnh_compressor_eval") {
        return;
    }
    if block != adapter::BLOCK || container.is_some() {
        return;
    }
    *container = Some(ModContainer {
        slots: 3,
        faces: [
            vec![0, 2],
            vec![0, 2],
            vec![0, 2],
            vec![0, 2],
            vec![0, 2],
            vec![0, 2],
        ],
        inputs: vec![0],
        outputs: vec![2],
        menu: Some("minecraft:furnace".to_owned()),
    });
}
