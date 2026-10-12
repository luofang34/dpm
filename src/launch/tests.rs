use super::*;

fn terminal(input: bool, output: bool) -> Terminal {
    Terminal { input, output }
}

#[test]
fn a_terminal_opens_the_console_and_macos_without_one_opens_the_app() {
    assert_eq!(
        opening(terminal(true, true), true).expect("console"),
        Opening::Console
    );
    assert_eq!(
        opening(terminal(true, true), false).expect("console"),
        Opening::Console
    );
    assert_eq!(
        opening(terminal(false, false), true).expect("app"),
        Opening::App
    );
    assert!(
        opening(terminal(false, false), false).is_err(),
        "no app on other platforms"
    );
    assert!(
        opening(terminal(true, false), true).is_err(),
        "half a terminal is a usage error"
    );
    assert!(
        opening(terminal(false, true), true).is_err(),
        "half a terminal is a usage error"
    );
}

#[test]
fn the_app_is_told_the_named_workspace_or_to_discover_from_here() {
    let cwd = Path::new("/work/here");
    assert_eq!(
        selection(None, Some(Path::new("/s.sqlite")), cwd),
        ["--database", "/s.sqlite"]
    );
    assert_eq!(
        selection(Some(Path::new("/p")), None, cwd),
        ["--project", "/p"]
    );
    assert_eq!(selection(None, None, cwd), ["--discover", "/work/here"]);
    assert_eq!(
        selection(Some(Path::new(".")), None, cwd),
        ["--project", "/work/here/."]
    );
    assert_eq!(
        selection(None, Some(Path::new("s.sqlite")), cwd),
        ["--database", "/work/here/s.sqlite"]
    );
}

#[test]
fn the_bundle_is_beside_the_binary_or_the_one_it_runs_inside() {
    assert_eq!(
        bundle_candidates(Path::new("/opt/dpm/bin/dpm")),
        [PathBuf::from("/opt/dpm/bin/DPMObserver.app")]
    );
    assert_eq!(
        bundle_candidates(Path::new(
            "/Applications/DPMObserver.app/Contents/Helpers/dpm"
        )),
        [
            PathBuf::from("/Applications/DPMObserver.app/Contents/Helpers/DPMObserver.app"),
            PathBuf::from("/Applications/DPMObserver.app"),
        ]
    );
}

#[test]
fn an_existing_bundle_is_opened_by_path_and_otherwise_by_identifier_and_a_failure_names_what_was_tried()
 {
    let root = std::env::temp_dir().join(format!("dpm-launch-{}", std::process::id()));
    let bundle = root.join("DPMObserver.app");
    std::fs::create_dir_all(&bundle).expect("bundle");
    let selection = vec!["--discover".to_owned(), "/here".to_owned()];
    let mut opened = Vec::new();
    open_app(
        &[root.join("missing.app"), bundle.clone()],
        &selection,
        |target, arguments| {
            opened.push((
                matches!(target, Target::Bundle(path) if *path == bundle),
                arguments.to_vec(),
            ));
            true
        },
    )
    .expect("opens by path");
    assert_eq!(opened, [(true, selection.clone())]);

    let mut by_identifier = false;
    open_app(&[root.join("missing.app")], &selection, |target, _| {
        by_identifier = matches!(target, Target::Identifier);
        by_identifier
    })
    .expect("opens by identifier");
    assert!(by_identifier);

    let mut fell_back = false;
    let failed = open_app(std::slice::from_ref(&bundle), &selection, |target, _| {
        fell_back |= matches!(target, Target::Identifier);
        false
    })
    .expect_err("the bundle beside dpm did not open");
    assert!(
        !fell_back && failed.to_string().contains("DPMObserver.app"),
        "never another installed copy: {failed}"
    );

    let refused =
        open_app(&[root.join("missing.app")], &selection, |_, _| false).expect_err("nothing opens");
    let message = refused.to_string();
    assert!(
        message.contains("missing.app") && message.contains(BUNDLE_IDENTIFIER),
        "{message}"
    );
    std::fs::remove_dir_all(&root).expect("cleanup");
}

#[test]
fn bare_dpm_is_status_with_json_without_probing_and_otherwise_the_console_the_app_or_a_usage_error()
{
    use crate::args::{Commands, OpenCommand};
    let status =
        default_command(true, || panic!("--json never probes the terminal"), true).expect("status");
    assert!(matches!(
        status,
        Commands::Status {
            no_simulation: false,
            calibrated: false
        }
    ));
    let console = default_command(false, || terminal(true, true), false).expect("console");
    assert!(matches!(console, Commands::Open(OpenCommand::Tui)));
    let app = default_command(false, || terminal(false, false), true).expect("app");
    assert!(matches!(app, Commands::Open(OpenCommand::Gui)));
    assert!(
        default_command(false, || terminal(false, false), false).is_err(),
        "no app off macOS"
    );
}
