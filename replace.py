import re
with open('crates/minter-core/src/raw_sniper.rs', 'r', encoding='utf-8') as f:
    content = f.read()

target = '''    // -- Clock fire --
    if let Some(at) = fire_at {
        let now = now_unix();
        if now < at {
            report(
                &reporter,
                MintEvent::phase(
                    "wait",
                    format!("Armed — firing in {}s (clock {at})", at - now),
                ),
            );
            if let Err(e) = sleep_until_fire(at, &cancel).await {
                return fail_all(signers, e);
            }
        }
    }'''

replacement = '''    // -- Clock fire --
    let reactive_engine = if rpc.ws_clients().is_empty() {
        None
    } else {
        Some(crate::reactive::ReactiveEngine::new(rpc.ws_clients()))
    };

    if let Some(at) = fire_at {
        let now = now_unix();
        if now < at {
            report(
                &reporter,
                MintEvent::phase(
                    "wait",
                    format!("Armed — firing in {}s (clock {at})", at - now),
                ),
            );
            if let Err(e) = sleep_until_fire(at, &cancel, &reactive_engine).await {
                return fail_all(signers, e);
            }
        }
    }'''

content = content.replace(target, replacement)
with open('crates/minter-core/src/raw_sniper.rs', 'w', encoding='utf-8') as f:
    f.write(content)
