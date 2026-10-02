#![cfg(test)]
//! The binary end to end: its one summary line and its distinct exit codes.

#[path = "adapter/support.rs"]
mod support;

use serde_json::{Value, json};
use std::process::Command;
use support::{Fixture, HANDSHAKE, emit, finished, init_event};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dpm-claude"))
}

#[test]
fn the_binary_prints_one_summary_line_and_distinct_exit_codes() {
    let fixture = Fixture::new();
    let script = fixture.script(
        "provider.sh",
        &[
            HANDSHAKE.to_string(),
            emit(&init_event()),
            emit(&finished(&json!([]))),
        ]
        .concat(),
    );
    let common = |mut command: Command, executor: &str| {
        command
            .args([
                "run",
                "--work",
                "TEST-A",
                "--executor",
                executor,
                "--prompt",
                "check",
                "--max-seconds",
                "30",
            ])
            .arg("--directory")
            .arg(&fixture.work_dir)
            .arg("--database")
            .arg(&fixture.database)
            .arg("--claude")
            .arg(&script);
        command.output().expect("run the binary")
    };
    let done = common(binary(), "agent:pilot");
    assert_eq!(
        done.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let summary: Value = serde_json::from_slice(&done.stdout).expect("one JSON line");
    assert_eq!(
        (
            summary["state"].as_str(),
            summary["evidence"].as_str(),
            summary["terminal_recorded"].as_bool()
        ),
        (Some("completed"), Some("result_success"), Some(true))
    );
    let refused = common(binary(), "agent:nobody");
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        refused.stdout.is_empty(),
        "standard output carries only the summary"
    );
    assert_eq!(
        binary().arg("--help").output().expect("help").status.code(),
        Some(0)
    );
    assert_eq!(
        binary()
            .args(["run", "--permission-mode", "bypassPermissions"])
            .output()
            .expect("bad")
            .status
            .code(),
        Some(2)
    );
}
