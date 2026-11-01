fn gtnh_lv_compressor_eval_register_commands(
    game: &mut crate::game::Game<'_>,
    commands: &mut Vec<NativeCommand>,
) {
    if !game.native_on("gtnh_lv_compressor_eval") {
        return;
    }
    commands.push(NativeCommand::op(
        "gtnhlvcompressor charge <x:integer> <y:integer> <z:integer> <eu:integer>",
        4,
    ));
    commands.push(NativeCommand::op(
        "gtnhlvcompressor trace <x:integer> <y:integer> <z:integer> <label:word>",
        4,
    ));
}