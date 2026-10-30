mod geometry;
mod icon;
mod gestures;
mod memory;
mod mover;
mod platform;
mod settings;
mod tray;
mod ui;
mod x11;

use std::process::ExitCode;

use settings::Settings;

const USAGE: &str = "\
Usage: souris-facile-pingouin [COMMAND]

Hold keys and move the mouse to move or resize the window under the pointer.

Commands:
  (none)               Run (X11 sessions only, for now)
  settings             Open the settings window
  displays [--preview] List the displays and their grid, ready to paste into
                       the settings; --preview shows the grids for 3 seconds
  -h, --help           Show this help
  -V, --version        Show the version
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let settings_path = settings::settings_path();
    let result = match args.first().map(String::as_str) {
        None => {
            if std::env::var_os("WAYLAND_DISPLAY").is_some()
                || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
            {
                eprintln!("This is a Wayland session: only X11 is supported for now.");
                eprintln!("On GNOME, use the Souris Facile extension instead.");
                return ExitCode::FAILURE;
            }
            eprintln!("Settings: {}", settings_path.display());
            x11::run(&settings_path, &settings::window_frames_path())
        }
        Some("settings") => x11::run_settings(&settings_path, &settings::window_frames_path()),
        Some("displays") => {
            let settings = Settings::load(&settings_path).unwrap_or_else(|e| {
                eprintln!("{e}");
                Settings::default()
            });
            x11::print_displays(&settings, args.iter().any(|a| a == "--preview"))
        }
        Some("-h" | "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("souris-facile-pingouin {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => {
            eprint!("Unknown command: {other}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
