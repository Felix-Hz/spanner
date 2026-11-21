//! ```text
//! spanner              open the window
//! spanner --record     background recorder (started by the OS at logon)
//! spanner --install    register recorder, app entry and shortcut
//! spanner --uninstall  remove all of the above and recorded history
//! ```

#![windows_subsystem = "windows"]

use spanner::error::{Error, Result};
use spanner::platform::{DialogKind, Integration, System, dialog};
use spanner::{app, recorder};

const UNINSTALL_PROMPT: &str = "Uninstall spanner?\n\n\
    This removes the background recorder, all recorded history, the Desktop shortcut and the entry in Installed apps. \
    Settings you changed stay as they are. To undo them first, open spanner and choose Restore defaults.";

fn run(arg: Option<&str>) -> Result<()> {
    // Installing and uninstalling change Task Scheduler and need elevation; the window and recorder don't
    // ask for it (the recorder is elevated by its task). Relaunch with one consent prompt.
    if let Some(a @ ("--install" | "--uninstall")) = arg
        && !System::is_elevated()
    {
        let code = System::run_elevated(a)?;
        std::process::exit(code as i32);
    }
    match arg {
        None => app::run(),
        Some("--record") => recorder::run(),
        Some("--install") => {
            System::install()?;
            dialog(
                "spanner is installed. The recorder runs whenever you're signed in, and there's a shortcut on your Desktop.",
                DialogKind::Info,
                false,
            );
            Ok(())
        }
        Some("--uninstall") => {
            if dialog(UNINSTALL_PROMPT, DialogKind::Warning, true) {
                let problems = System::uninstall()?;
                System::schedule_profile_cleanup();
                match problems.as_slice() {
                    [] => dialog("spanner is uninstalled.", DialogKind::Info, false),
                    p => dialog(
                        &format!("spanner is uninstalled, but some items couldn't be removed:\n\n{}", p.join("\n")),
                        DialogKind::Warning,
                        false,
                    ),
                };
            }
            Ok(())
        }
        Some(other) => Err(Error(format!("Unknown option '{other}'."))),
    }
}

fn main() {
    let arg = std::env::args().nth(1);
    if let Err(e) = run(arg.as_deref()) {
        // The recorder is headless; the scheduler restarts it.
        if arg.as_deref() != Some("--record") {
            dialog(&e.0, DialogKind::Error, false);
        }
        std::process::exit(1);
    }
}
