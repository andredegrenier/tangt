//! The pipeline must never panic, no matter what the editor contains.

use proptest::prelude::*;
use tangt_core::parser::parse;
use tangt_core::resolve::resolve;

/// Fragments biased toward tangt-looking input so we exercise deep paths, not
/// just the error-recovery front door.
fn tangt_like() -> impl Strategy<Value = String> {
    let word = prop_oneof![
        Just("task".to_string()),
        Just("milestone".to_string()),
        Just("section".to_string()),
        Just("after".to_string()),
        Just("until".to_string()),
        Just("exclude".to_string()),
        Just("weekends".to_string()),
        Just("title".to_string()),
        Just("axis".to_string()),
        Just("today".to_string()),
        Just("crit".to_string()),
        Just("a".to_string()),
        Just("b2".to_string()),
        Just("\"Label\"".to_string()),
        Just("\"say \\\"hi\\\"\"".to_string()),
        Just("2026-08-10".to_string()),
        Just("9999-99-99".to_string()),
        Just("5d".to_string()),
        Just("2w".to_string()),
        Just("60%".to_string()),
        Just("#4fa3ff".to_string()),
        Just("#zz".to_string()),
        Just(":".to_string()),
        Just(",".to_string()),
        Just("..".to_string()),
        Just("+".to_string()),
        Just("-".to_string()),
        Just("//c".to_string()),
        Just("\n".to_string()),
        Just(" ".to_string()),
    ];
    prop::collection::vec(word, 0..60).prop_map(|words| words.join(" "))
}

proptest! {
    #[test]
    fn parse_and_resolve_never_panic_on_tangt_like(src in tangt_like()) {
        let ast = parse(&src);
        let _ = resolve(&ast, 20675);
    }

    #[test]
    fn parse_and_resolve_never_panic_on_arbitrary(src in ".{0,200}") {
        let ast = parse(&src);
        let _ = resolve(&ast, 20675);
    }
}
