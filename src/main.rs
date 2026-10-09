//! niri-appbeam — run-or-raise for the niri Wayland compositor.
//!
//! Behaviour, given a launch command (and optionally an app-id regex):
//!   - no matching window           -> launch the command
//!   - matching window, not focused -> focus it (oldest one first)
//!   - matching window is focused   -> switch back to the previously focused window

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};

use clap::Parser;
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Request, Response, Window};
use regex::Regex;

/// Command-line arguments.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Run-or-raise for niri: launch an app, focus it, or jump back.",
    after_help = "Examples:\n  \
        niri-appbeam firefox\n  \
        niri-appbeam obsidian\n  \
        niri-appbeam --app-id '^dev\\.yazi$' ghostty --class=dev.yazi -e yazi"
)]
struct Cli {
    /// Regex matched against the window app-id (see `niri msg windows`).
    /// If omitted, the command name is matched against the app-id (case-insensitive):
    /// either the whole app-id or its last dot-separated part, so `obsidian` matches
    /// `md.obsidian.Obsidian`.
    #[arg(short, long, value_name = "REGEX")]
    app_id: Option<String>,

    /// Command (with arguments) used to launch the app when no window matches.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

/// How windows are matched against what the user asked for.
#[derive(Debug)]
enum Matcher {
    /// Derived from the command name: compares the whole app-id or its last
    /// dot-separated segment, case-insensitively.
    Name(String),
    /// Explicit `--app-id` regex.
    Regex(Regex),
}

impl Matcher {
    /// Derive a matcher from the command, e.g. "/usr/bin/firefox" -> Name("firefox").
    fn from_command(cmd: &str) -> Self {
        let name = Path::new(cmd)
            .file_name()
            .map_or(cmd.into(), |n| n.to_string_lossy());
        Matcher::Name(name.to_lowercase())
    }

    fn matches(&self, app_id: &str) -> bool {
        match self {
            Matcher::Regex(re) => re.is_match(app_id),
            Matcher::Name(name) => {
                let app_id = app_id.to_lowercase();
                // rsplit always yields at least one item, so unwrap_or is just defensive.
                let last = app_id.rsplit('.').next().unwrap_or(&app_id);
                app_id == *name || last == name
            }
        }
    }
}

/// What niri-appbeam decided to do after looking at the window list.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// No matching window exists: start the app.
    Launch,
    /// A matching window is already focused: go back to the previous window.
    FocusPrevious,
    /// A matching window exists but is not focused: focus the window with this id.
    Focus(u64),
}

/// Pure decision logic, kept separate from IPC so it can be unit-tested.
fn decide(windows: &[Window], matcher: &Matcher) -> Decision {
    // Keep only windows that have an app-id the matcher accepts.
    let mut matching: Vec<&Window> = windows
        .iter()
        .filter(|w| w.app_id.as_deref().is_some_and(|id| matcher.matches(id)))
        .collect();

    // Nothing matches -> the app isn't running.
    if matching.is_empty() {
        return Decision::Launch;
    }

    // The app is already focused -> toggle back.
    if matching.iter().any(|w| w.is_focused) {
        return Decision::FocusPrevious;
    }

    // Otherwise focus the oldest matching window (lowest id) for predictable behaviour.
    matching.sort_by_key(|w| w.id);
    Decision::Focus(matching[0].id)
}

/// Send one request to niri and unwrap both transport and niri-side errors.
fn request(socket: &mut Socket, req: Request) -> Result<Response, String> {
    socket
        .send(req)
        .map_err(|e| format!("niri IPC error: {e}"))? // could not talk to the socket
        .map_err(|e| format!("niri returned an error: {e}")) // niri rejected the request
}

/// Send an action to niri.
fn action(socket: &mut Socket, action: Action) -> Result<(), String> {
    request(socket, Request::Action(action)).map(|_| ())
}

fn run(cli: Cli) -> Result<(), String> {
    // Use the explicit regex if given, otherwise derive the match from the command name.
    let matcher = match &cli.app_id {
        Some(re) => {
            Matcher::Regex(Regex::new(re).map_err(|e| format!("invalid app-id regex: {e}"))?)
        }
        None => Matcher::from_command(&cli.command[0]),
    };

    // Connect to niri via $NIRI_SOCKET.
    let mut socket = Socket::connect().map_err(|e| format!("cannot connect to niri: {e}"))?;

    // Fetch the current window list.
    let windows = match request(&mut socket, Request::Windows)? {
        Response::Windows(w) => w,
        other => return Err(format!("unexpected reply from niri: {other:?}")),
    };

    match decide(&windows, &matcher) {
        Decision::Launch => {
            // Replace this process with the app, like `exec` in a shell script.
            // exec() only returns if launching failed.
            let err = Command::new(&cli.command[0]).args(&cli.command[1..]).exec();
            Err(format!("failed to launch {:?}: {err}", cli.command[0]))
        }
        Decision::FocusPrevious => action(&mut socket, Action::FocusWindowPrevious {}),
        Decision::Focus(id) => action(&mut socket, Action::FocusWindow { id }),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("niri-appbeam: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal Window via JSON so tests don't depend on every field of the struct.
    fn win(id: u64, app_id: Option<&str>, focused: bool) -> Window {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": null,
            "app_id": app_id,
            "pid": null,
            "workspace_id": null,
            "is_focused": focused,
            "is_floating": false,
            "is_urgent": false,
            "layout": {
                "pos_in_scrolling_layout": null,
                "tile_size": [0.0, 0.0],
                "window_size": [0, 0],
                "tile_pos_in_workspace_view": null,
                "window_offset_in_tile": [0.0, 0.0]
            },
            "focus_timestamp": null
        }))
        .expect("Window JSON shape changed in niri-ipc; update test helper")
    }

    fn ff() -> Matcher {
        Matcher::from_command("firefox")
    }

    #[test]
    fn launches_when_no_match() {
        let ws = [win(1, Some("ghostty"), true), win(2, None, false)];
        assert_eq!(decide(&ws, &ff()), Decision::Launch);
    }

    #[test]
    fn focuses_oldest_unfocused_match() {
        let ws = [
            win(9, Some("firefox"), false),
            win(3, Some("firefox"), false),
            win(1, Some("ghostty"), true),
        ];
        assert_eq!(decide(&ws, &ff()), Decision::Focus(3));
    }

    #[test]
    fn goes_back_when_already_focused() {
        let ws = [win(3, Some("firefox"), false), win(9, Some("firefox"), true)];
        assert_eq!(decide(&ws, &ff()), Decision::FocusPrevious);
    }

    #[test]
    fn name_matcher_rules() {
        let m = |cmd: &str, id: &str| Matcher::from_command(cmd).matches(id);
        assert!(m("firefox", "firefox"));
        assert!(m("/usr/bin/firefox", "firefox")); // path stripped
        assert!(m("obsidian", "md.obsidian.Obsidian")); // last segment, case-insensitive
        assert!(m("ghostty", "com.mitchellh.ghostty"));
        assert!(m("org.mozilla.firefox", "org.mozilla.firefox")); // whole id
        assert!(!m("firefox", "firefox-developer-edition")); // no substring matches
        assert!(!m("fire", "firefox"));
    }

    #[test]
    fn regex_matcher() {
        let m = Matcher::Regex(Regex::new(r"^dev\.yazi$").unwrap());
        assert!(m.matches("dev.yazi"));
        assert!(!m.matches("com.mitchellh.ghostty"));
    }
}
