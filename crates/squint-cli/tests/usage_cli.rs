//! Argument errors exit 64 on every command (table-manners 6), never clap's 2, which squint's
//! `check` uses for `unsure`.
#![allow(clippy::unwrap_used)]

use std::process::Command;

#[test]
fn an_unknown_flag_exits_64_on_every_command() {
    for command in [
        &["check"][..],
        &["describe"],
        &["estimate"],
        &["serve"],
        &["playground"],
        &["report", "view"],
        &["eval", "run"],
        &["sets", "check"],
        &["catalog", "list"],
        &["rules", "show"],
        &["tools", "list"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_squint"))
            .args(command)
            .arg("--no-such-flag")
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(64),
            "{command:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}
