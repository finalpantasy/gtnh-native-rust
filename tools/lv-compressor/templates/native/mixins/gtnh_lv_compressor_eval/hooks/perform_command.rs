fn gtnh_lv_compressor_eval_perform_command(
    game: &mut crate::game::Game<'_>,
    command: &str,
    ran: &mut bool,
) {
    use crate::game::gtnh_lv_compressor_eval::adapter;
    use horizon_client::text::Text;

    if !game.native_on("gtnh_lv_compressor_eval") || *ran {
        return;
    }
    let mut words = command.split_whitespace();
    if words.next() != Some("gtnhlvcompressor") {
        return;
    }
    let Some(action) = words.next() else {
        return;
    };
    if action != "charge" && action != "trace" {
        return;
    }
    *ran = true;
    let args = words.collect::<Vec<_>>();

    let usage = if action == "charge" {
        "Usage: /gtnhlvcompressor charge <x> <y> <z> <eu>"
    } else {
        "Usage: /gtnhlvcompressor trace <x> <y> <z> <nominal|shortage|blocked_output>"
    };
    if args.len() != 4 {
        game.native_feedback(Text::literal(usage));
        return;
    }
    let (Some(x), Some(y), Some(z)) = (
        args[0].parse::<i32>().ok(),
        args[1].parse::<i32>().ok(),
        args[2].parse::<i32>().ok(),
    ) else {
        game.native_feedback(Text::literal(usage));
        return;
    };

    let charge_eu = if action == "charge" {
        let Ok(amount) = args[3].parse::<i64>() else {
            game.native_feedback(Text::literal(
                "Fixture charge must be an integer from 1 through 2048 EU.",
            ));
            return;
        };
        if !(1..=2048).contains(&amount) {
            game.native_feedback(Text::literal(
                "Fixture charge must be an integer from 1 through 2048 EU.",
            ));
            return;
        }
        Some(amount as u64)
    } else {
        None
    };

    if game.native_block([x, y, z]) != adapter::BLOCK {
        game.native_feedback(Text::literal(
            "No LV compressor evaluation block exists at those coordinates.",
        ));
        return;
    }
    let result = game.native_with_mod_block_entity([x, y, z], |entity| {
        if entity.id != adapter::BLOCK {
            return Err("the target is not the LV compressor evaluation block".to_owned());
        }
        match (action, charge_eu) {
            ("charge", Some(amount)) => adapter::fixture_charge(entity, amount).map(|accepted| {
                format!(
                    "Applied {accepted} EU of fixture power."
                )
            }),
            ("trace", None) => adapter::mark_trace(entity, args[3])
                .map(|()| "Marked the LV compressor trace fixture.".to_owned()),
            _ => Err("invalid LV compressor fixture command".to_owned()),
        }
    });

    match (action, result) {
        ("charge", None) | ("trace", None) => game.native_feedback(Text::literal(
            "No LV compressor mod block entity exists at those coordinates.",
        )),
        ("charge", Some(Ok(message))) | ("trace", Some(Ok(message))) => {
            game.native_feedback(Text::literal(&message));
        }
        ("charge", Some(Err(error))) => game.native_feedback(Text::literal(&format!(
            "LV compressor fixture charge rejected: {error}",
        ))),
        ("trace", Some(Err(error))) => game.native_feedback(Text::literal(&format!(
            "LV compressor trace marking rejected: {error}",
        ))),
        _ => {}
    }
}