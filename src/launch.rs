//! One entry point: `dpm` alone opens the terminal console in a terminal and, on macOS without one, the
//! DPMObserver app; `dpm tui` and `dpm gui` choose explicitly. Opening the app issues no application command:
//! it hands the app the project selection, and the app opens that workspace itself.

use crate::args::{Commands, OpenCommand};
use crate::error::CliError;
use std::path::{Path, PathBuf};

/// The app's bundle identifier, the last place `dpm gui` looks for it.
pub(crate) const BUNDLE_IDENTIFIER: &str = "org.dpm.observer";
const BUNDLE_NAME: &str = "DPMObserver.app";

/// Whether standard input and output are terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Terminal {
    pub(crate) input: bool,
    pub(crate) output: bool,
}

impl Terminal {
    pub(crate) fn probe() -> Self {
        use std::io::IsTerminal;
        Self {
            input: std::io::stdin().is_terminal(),
            output: std::io::stdout().is_terminal(),
        }
    }
}

/// What `dpm` with no subcommand and without --json opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Opening {
    Console,
    App,
}

/// What `dpm` with no subcommand runs: the status envelope with --json, whatever the terminal, never probing it;
/// otherwise the console or the app as `opening` decides from the probed terminal.
pub(crate) fn default_command(
    json: bool,
    probe: impl FnOnce() -> Terminal,
    macos: bool,
) -> Result<Commands, CliError> {
    if json {
        return Ok(Commands::Status {
            no_simulation: false,
            calibrated: false,
        });
    }
    Ok(match opening(probe(), macos)? {
        Opening::Console => Commands::Open(OpenCommand::Tui),
        Opening::App => Commands::Open(OpenCommand::Gui),
    })
}

/// The console on a terminal, the app on macOS when neither end is a terminal, otherwise a usage error.
pub(crate) fn opening(terminal: Terminal, macos: bool) -> Result<Opening, CliError> {
    match (terminal.input, terminal.output) {
        (true, true) => Ok(Opening::Console),
        (false, false) if macos => Ok(Opening::App),
        _ => Err(CliError::Input(
            "dpm without a subcommand opens the console in a terminal, or the app on macOS without one; \
             use `dpm tui`, `dpm gui` or a subcommand"
                .into(),
        )),
    }
}

/// The workspace the app is told to open: the one the command line named, or discovery from `cwd`.
pub(crate) fn selection(
    project: Option<&Path>,
    database: Option<&Path>,
    cwd: &Path,
) -> Vec<String> {
    let (flag, path) = match (database, project) {
        (Some(database), _) => ("--database", database),
        (None, Some(project)) => ("--project", project),
        (None, None) => ("--discover", cwd),
    };
    // The app is started by the system with another working directory, so a relative path is made absolute here.
    vec![flag.to_owned(), cwd.join(path).display().to_string()]
}

/// Where the app may be: beside the running binary, or the bundle the binary runs inside.
pub(crate) fn bundle_candidates(executable: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(directory) = executable.parent() {
        found.push(directory.join(BUNDLE_NAME));
        // Contents/Helpers/dpm inside an app bundle: the bundle is three levels up.
        let bundle = directory.parent().and_then(Path::parent);
        if directory.ends_with("Contents/Helpers")
            && let Some(bundle) = bundle.filter(|b| b.extension().is_some_and(|e| e == "app"))
        {
            found.push(bundle.to_path_buf());
        }
    }
    found
}

/// How the app is started: by its path or, failing that, by its bundle identifier.
pub(crate) enum Target {
    Bundle(PathBuf),
    Identifier,
}

/// Open the app on `selection`, trying each candidate that exists and then the bundle identifier; `start` runs the
/// system's opener and says whether it succeeded.
pub(crate) fn open_app(
    candidates: &[PathBuf],
    selection: &[String],
    mut start: impl FnMut(&Target, &[String]) -> bool,
) -> Result<(), CliError> {
    // A bundle that belongs with this dpm is opened or the launch fails; the identifier, which may name another
    // installed copy, is used only when no such bundle exists.
    if let Some(bundle) = candidates.iter().find(|path| path.is_dir()) {
        if start(&Target::Bundle(bundle.clone()), selection) {
            return Ok(());
        }
        return Err(CliError::Input(format!(
            "could not open {}",
            bundle.display()
        )));
    }
    if start(&Target::Identifier, selection) {
        return Ok(());
    }
    let tried: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    Err(CliError::Input(format!(
        "could not open {BUNDLE_NAME}: tried {} and bundle identifier {BUNDLE_IDENTIFIER}",
        if tried.is_empty() {
            "no path".to_owned()
        } else {
            tried.join(", ")
        }
    )))
}

/// The system opener: `open -n` with the app's own arguments after `--args`.
pub(crate) fn start_with_open(target: &Target, selection: &[String]) -> bool {
    let mut command = std::process::Command::new("/usr/bin/open");
    command.arg("-n");
    match target {
        Target::Bundle(path) => command.arg(path),
        Target::Identifier => command.args(["-b", BUNDLE_IDENTIFIER]),
    };
    command.arg("--args").args(selection);
    match command.status() {
        Ok(status) => status.success(),
        Err(error) => {
            tracing::debug!(%error, "the system opener could not run");
            false
        }
    }
}

/// `dpm gui`: open the app on the selected workspace.
pub(crate) fn gui_blocking(
    project: Option<&Path>,
    database: Option<&Path>,
    cwd: &Path,
) -> Result<(), CliError> {
    if !cfg!(target_os = "macos") {
        return Err(CliError::Input(
            "dpm gui opens the macOS app; on this platform use `dpm tui`".into(),
        ));
    }
    let executable =
        std::env::current_exe().map_err(crate::error::io_error("locate the running dpm", "dpm"))?;
    // A dpm reached through a link is looked up from where it really is.
    let executable = executable.canonicalize().unwrap_or(executable);
    open_app(
        &bundle_candidates(&executable),
        &selection(project, database, cwd),
        start_with_open,
    )
}

#[cfg(test)]
mod tests;
