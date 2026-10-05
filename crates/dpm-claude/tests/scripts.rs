//! Provider scripts the adapter tests write must execute even while other test threads spawn children.
#![cfg(test)]

#[path = "adapter/support.rs"]
mod support;

use support::Fixture;

/// Scripts written while other threads fork and exec children must still execute: on Linux a
/// write descriptor a sibling's child inherited would fail the exec with ETXTBSY.
#[test]
fn scripts_execute_while_other_threads_spawn_children() {
    let threads: Vec<_> = (0..8)
        .map(|thread| {
            std::thread::spawn(move || {
                let fixture = Fixture::new();
                for round in 0..25 {
                    let path = fixture.script(&format!("probe-{thread}-{round}.sh"), "exit 0\n");
                    let status = std::process::Command::new(&path)
                        .current_dir(fixture.directory.path())
                        .status()
                        .expect("execute the script just written");
                    assert!(status.success(), "{}", path.display());
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("a script thread");
    }
}
