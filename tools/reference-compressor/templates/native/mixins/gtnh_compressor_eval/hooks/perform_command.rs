fn gtnh_compressor_eval_perform_command(
    game: &mut crate::game::Game<'_>,
    command: &str,
    ran: &mut bool,
) {
    use crate::game::gtnh_compressor_eval::adapter;
    use horizon_client::text::Text;

    if !game.native_on("gtnh_compressor_eval") || *ran {
        return;
    }
    let mut words = command.split_whitespace();
    if words.next() != Some("gtnhcompressor") || words.next() != Some("charge") {
        return;
    }
    *ran = true;
    let (x, y, z) = (
        words.next().and_then(|value| value.parse::<i32>().ok()),
        words.next().and_then(|value| value.parse::<i32>().ok()),
        words.next().and_then(|value| value.parse::<i32>().ok()),
    );
    if words.next().is_some() {
        game.native_feedback(Text::literal(
            "Usage: /gtnhcompressor charge <x> <y> <z>",
        ));
        return;
    }
    let (Some(x), Some(y), Some(z)) = (x, y, z) else {
        game.native_feedback(Text::literal(
            "Usage: /gtnhcompressor charge <x> <y> <z>",
        ));
        return;
    };

    if game.native_block([x, y, z]) != adapter::BLOCK {
        game.native_feedback(Text::literal(
            "No evaluation compressor exists at those coordinates.",
        ));
        return;
    }
    let result = game.native_with_mod_block_entity([x, y, z], |entity| {
        if entity.id != adapter::BLOCK {
            return Err("the target is not the evaluation compressor".to_owned());
        }
        adapter::test_charge_once(entity)
    });
    match result {
        None => game.native_feedback(Text::literal(
            "No mod block entity exists at those coordinates.",
        )),
        Some(Ok(600)) => game.native_feedback(Text::literal(
            "Applied the one-time 600 EU prototype test charge.",
        )),
        Some(Ok(0)) => game.native_feedback(Text::literal(
            "The one-time prototype test charge was already recorded; added 0 EU.",
        )),
        Some(Ok(accepted)) => game.native_feedback(Text::literal(&format!(
            "Accepted {accepted} EU for the prototype test charge.",
        ))),
        Some(Err(error)) => game.native_feedback(Text::literal(&format!(
            "Prototype charge rejected: {error}",
        ))),
    }
}
