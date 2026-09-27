use super::*;

#[test]
fn a_termination_signal_sets_the_flag_instead_of_ending_the_process() {
    let interrupts = Interrupts::register().expect("register");
    assert!(!interrupts.received());
    signal_hook::low_level::raise(SIGTERM).expect("raise");
    assert!(interrupts.received(), "the loop must see the request");
    drop(interrupts);
}
