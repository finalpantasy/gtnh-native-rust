fn gtnh_compressor_eval_block_entity(
    game: &mut crate::game::Game<'_>,
    block: &str,
    entity: &mut Option<(String, horizon_world::nbt::Compound)>,
) {
    use crate::game::gtnh_compressor_eval::adapter;

    if !game.native_on("gtnh_compressor_eval") {
        return;
    }
    if block != adapter::BLOCK || entity.is_some() {
        return;
    }
    match adapter::new_entity() {
        Ok(created) => *entity = Some((adapter::BLOCK.to_owned(), created.nbt)),
        Err(error) => eprintln!("GTNH_COMPRESSOR_CREATE_FAIL error={error}"),
    }
}
