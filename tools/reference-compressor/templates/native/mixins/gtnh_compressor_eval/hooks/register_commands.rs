fn gtnh_compressor_eval_register_commands(
    game: &mut crate::game::Game<'_>,
    commands: &mut Vec<NativeCommand>,
) {
    if !game.native_on("gtnh_compressor_eval") {
        return;
    }
    commands.push(NativeCommand::op(
        "gtnhcompressor charge <x:integer> <y:integer> <z:integer>",
        4,
    ));
}
