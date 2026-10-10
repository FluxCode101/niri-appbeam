//! niri-appbeam — run-or-raise for the niri Wayland compositor.
//!
//! Behaviour, given a launch command (and optionally an app-id regex):
//!   - no matching window           -> launch the command
//!   - matching window, not focused -> focus it (oldest one first)
//!   - matching window is focused   -> switch back to the previously focused window
//!
//! With `--tab` / `--tab-prev`:
//!   - If no command is given: cycles through all windows of the currently active app
//!     (in creation order, wrapping around; new windows are automatically appended).
//!   - If a command is given: focuses or cycles matching windows of that app.
//!
//! Terminal subcommands (e.g. `ghostty -e yazi`, `foot yazi`):
//!   - Automatically detected and isolated with `--class=appbeam.<app>` on launch.
//!   - Automatically matched against the child app without clashing with general terminal windows.

use std::io::{ErrorKind, IsTerminal};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use clap::Parser;
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Request, Response, Window};
use regex::Regex;

/// Command-line arguments.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Run-or-raise for niri: launch an app, focus it, jump back, or tab through windows.",
    after_help = "Examples:\n  \
        niri-appbeam firefox\n  \
        niri-appbeam -tab                 # cycle through all windows of the current app\n  \
        niri-appbeam -tabprev             # cycle backwards through current app windows\n  \
        niri-appbeam -tab firefox         # cycle through firefox windows (or launch)\n  \
        niri-appbeam ghostty -e yazi      # launch/switch to yazi inside ghostty (auto-class)\n  \
        niri-appbeam --app-id '^Spotify$' spotify-launcher"
)]
struct Cli {
    /// Regex matched against the window app-id (see `niri msg windows`).
    /// If omitted, the command name is matched against the app-id (case-insensitive):
    /// either the whole app-id or its last dot-separated part, so `obsidian` matches
    /// `md.obsidian.Obsidian`.
    #[arg(short, long, value_name = "REGEX")]
    app_id: Option<String>,

    /// Cycle to the next window of the app (creation order, wraps around).
    /// If no command is provided, cycles windows of the currently active app.
    /// Also accepted as `-tab`.
    #[arg(long, conflicts_with = "tab_prev")]
    tab: bool,

    /// Like --tab, but cycles in reverse order. Also accepted as `-tabprev`.
    #[arg(long, visible_alias = "tabprev")]
    tab_prev: bool,

    /// Command (with arguments) used to launch the app when no window matches.
    /// Optional when using --tab or --tab-prev.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

/// Accept the single-dash spellings `-tab` / `-tabprev` for our own options.
///
/// clap would otherwise read `-tab` as the short flags `-t -a -b`. Only arguments
/// before the launch command are rewritten, so the app's own flags are never touched.
fn normalize_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut iter = args.into_iter();
    let mut out: Vec<String> = iter.next().into_iter().collect(); // program name
    let mut takes_value = false;
    for arg in iter.by_ref() {
        if takes_value {
            takes_value = false;
            out.push(arg);
            continue;
        }
        match arg.as_str() {
            "-tab" => out.push("--tab".into()),
            "-tabprev" | "-tab-prev" => out.push("--tab-prev".into()),
            "-a" | "--app-id" => {
                takes_value = true;
                out.push(arg);
            }
            "--" => {
                out.push(arg);
                break;
            }
            a if a.starts_with('-') => out.push(arg),
            // First positional = the launch command; everything after belongs to it.
            _ => {
                out.push(arg);
                break;
            }
        }
    }
    out.extend(iter);
    out
}

/// What to do when an app is already focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Jump back to the previously focused window (default).
    Toggle,
    /// Cycle forward through the app's windows (`--tab`).
    Next,
    /// Cycle backward through the app's windows (`--tab-prev`).
    Prev,
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

/// Detects if `command` is a terminal emulator launching a subcommand (e.g. `ghostty -e yazi`).
/// If so, returns:
/// - `child_app_name`: used for matching (e.g. `yazi` matches `appbeam.yazi` or `yazi`)
/// - `modified_cmd`: terminal command with injected `--class=appbeam.<app>` (or `--app-id`)
fn detect_terminal_subcommand(command: &[String]) -> Option<(String, Vec<String>)> {
    if command.is_empty() {
        return None;
    }

    let bin = Path::new(&command[0])
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| command[0].to_lowercase());

    match bin.as_str() {
        "ghostty" => {
            if command.iter().any(|a| a.starts_with("--class")) {
                return None;
            }
            let mut sub_idx = None;
            for (i, arg) in command.iter().enumerate().skip(1) {
                if arg == "-e" || arg == "--" {
                    if i + 1 < command.len() {
                        sub_idx = Some(i + 1);
                    }
                    break;
                } else if let Some(sub) = arg.strip_prefix("-e=")
                    && !sub.is_empty()
                {
                    let sub_name = extract_app_name(sub);
                    let class_id = format!("appbeam.{sub_name}");
                    let mut new_cmd = command.to_vec();
                    new_cmd.insert(1, format!("--class={class_id}"));
                    return Some((sub_name, new_cmd));
                }
            }

            if let Some(idx) = sub_idx {
                let sub_name = extract_app_name(&command[idx]);
                let class_id = format!("appbeam.{sub_name}");
                let mut new_cmd = command.to_vec();
                new_cmd.insert(1, format!("--class={class_id}"));
                Some((sub_name, new_cmd))
            } else {
                None
            }
        }
        "foot" => {
            if command.iter().any(|a| a.starts_with("--app-id")) {
                return None;
            }
            let mut sub_idx = None;
            for (i, arg) in command.iter().enumerate().skip(1) {
                if arg == "-e" || arg == "--" {
                    if i + 1 < command.len() {
                        sub_idx = Some(i + 1);
                    }
                    break;
                }
            }
            if sub_idx.is_none() {
                for (i, arg) in command.iter().enumerate().skip(1) {
                    if !arg.starts_with('-') {
                        sub_idx = Some(i);
                        break;
                    }
                }
            }
            if let Some(idx) = sub_idx {
                let sub_name = extract_app_name(&command[idx]);
                let class_id = format!("appbeam.{sub_name}");
                let mut new_cmd = command.to_vec();
                new_cmd.insert(1, format!("--app-id={class_id}"));
                Some((sub_name, new_cmd))
            } else {
                None
            }
        }
        "alacritty" => {
            if command.iter().any(|a| a.starts_with("--class")) {
                return None;
            }
            let mut sub_idx = None;
            for (i, arg) in command.iter().enumerate().skip(1) {
                if arg == "-e" || arg == "--command" || arg == "--" {
                    if i + 1 < command.len() {
                        sub_idx = Some(i + 1);
                    }
                    break;
                }
            }
            if let Some(idx) = sub_idx {
                let sub_name = extract_app_name(&command[idx]);
                let class_id = format!("appbeam.{sub_name}");
                let mut new_cmd = command.to_vec();
                new_cmd.insert(1, format!("--class={class_id}"));
                Some((sub_name, new_cmd))
            } else {
                None
            }
        }
        "kitty" => {
            if command.iter().any(|a| a.starts_with("--class") || a.starts_with("--app-id")) {
                return None;
            }
            let mut sub_idx = None;
            for (i, arg) in command.iter().enumerate().skip(1) {
                if arg == "-e" || arg == "--" {
                    if i + 1 < command.len() {
                        sub_idx = Some(i + 1);
                    }
                    break;
                }
            }
            if sub_idx.is_none() {
                for (i, arg) in command.iter().enumerate().skip(1) {
                    if !arg.starts_with('-') {
                        sub_idx = Some(i);
                        break;
                    }
                }
            }
            if let Some(idx) = sub_idx {
                let sub_name = extract_app_name(&command[idx]);
                let class_id = format!("appbeam.{sub_name}");
                let mut new_cmd = command.to_vec();
                new_cmd.insert(1, format!("--class={class_id}"));
                Some((sub_name, new_cmd))
            } else {
                None
            }
        }
        "wezterm" => {
            if command.iter().any(|a| a.starts_with("--class")) {
                return None;
            }
            let mut sub_idx = None;
            for (i, arg) in command.iter().enumerate().skip(1) {
                if arg == "-e" || arg == "--" {
                    if i + 1 < command.len() {
                        sub_idx = Some(i + 1);
                    }
                    break;
                }
            }
            if let Some(idx) = sub_idx {
                let sub_name = extract_app_name(&command[idx]);
                let class_id = format!("appbeam.{sub_name}");
                let mut new_cmd = command.to_vec();
                new_cmd.insert(1, format!("--class={class_id}"));
                Some((sub_name, new_cmd))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_app_name(cmd: &str) -> String {
    let name = Path::new(cmd)
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| cmd.into());
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if sanitized.is_empty() {
        "app".to_string()
    } else {
        sanitized.to_lowercase()
    }
}

/// What niri-appbeam decided to do after looking at the window list.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// No matching window exists: start the app with command.
    Launch(Vec<String>),
    /// A matching window is already focused: go back to the previous window.
    FocusPrevious,
    /// Focus the window with this id.
    Focus(u64),
    /// Nothing to do (e.g. single window already focused in tab mode, or no focused window).
    Nothing,
}

/// Pure decision logic when a specific command / matcher is targeted.
fn decide(
    windows: &[Window],
    matcher: &Matcher,
    mode: Mode,
    launch_cmd: Vec<String>,
) -> Decision {
    let mut matching: Vec<&Window> = windows
        .iter()
        .filter(|w| w.app_id.as_deref().is_some_and(|id| matcher.matches(id)))
        .collect();
    matching.sort_by_key(|w| w.id);

    if matching.is_empty() {
        return Decision::Launch(launch_cmd);
    }

    let Some(pos) = matching.iter().position(|w| w.is_focused) else {
        return Decision::Focus(matching[0].id);
    };

    let n = matching.len();
    match mode {
        Mode::Toggle => Decision::FocusPrevious,
        _ if n == 1 => Decision::Nothing,
        Mode::Next => Decision::Focus(matching[(pos + 1) % n].id),
        Mode::Prev => Decision::Focus(matching[(pos + n - 1) % n].id),
    }
}

/// Pure decision logic for active window tabbing (no command specified).
/// Finds the currently focused window, gathers all windows with the same `app_id`,
/// and cycles through them in creation order (sorted by id, newly opened windows appended).
fn decide_active_tab(windows: &[Window], mode: Mode) -> Decision {
    let Some(focused) = windows.iter().find(|w| w.is_focused) else {
        return Decision::Nothing;
    };

    let Some(app_id) = focused.app_id.as_deref() else {
        return Decision::Nothing;
    };

    let mut matching: Vec<&Window> = windows
        .iter()
        .filter(|w| w.app_id.as_deref() == Some(app_id))
        .collect();
    matching.sort_by_key(|w| w.id);

    let n = matching.len();
    if n <= 1 {
        return Decision::Nothing;
    }

    let pos = matching.iter().position(|w| w.id == focused.id).unwrap();
    match mode {
        Mode::Next => Decision::Focus(matching[(pos + 1) % n].id),
        Mode::Prev => Decision::Focus(matching[(pos + n - 1) % n].id),
        Mode::Toggle => Decision::FocusPrevious,
    }
}

/// Send one request to niri and unwrap both transport and niri-side errors.
fn request(socket: &mut Socket, req: Request) -> Result<Response, String> {
    socket
        .send(req)
        .map_err(|e| format!("niri IPC error: {e}"))?
        .map_err(|e| format!("niri returned an error: {e}"))
}

/// Send an action to niri.
fn action(socket: &mut Socket, action: Action) -> Result<(), String> {
    request(socket, Request::Action(action)).map(|_| ())
}

fn run(cli: Cli) -> Result<(), String> {
    let mode = if cli.tab {
        Mode::Next
    } else if cli.tab_prev {
        Mode::Prev
    } else {
        Mode::Toggle
    };

    if cli.command.is_empty() && !cli.tab && !cli.tab_prev {
        return Err("either <COMMAND> or --tab / --tab-prev is required".into());
    }

    // Connect to niri via $NIRI_SOCKET.
    let mut socket = Socket::connect().map_err(|e| format!("cannot connect to niri: {e}"))?;

    // Fetch the current window list.
    let windows = match request(&mut socket, Request::Windows)? {
        Response::Windows(w) => w,
        other => return Err(format!("unexpected reply from niri: {other:?}")),
    };

    let decision = if cli.command.is_empty() {
        if let Some(ref re) = cli.app_id {
            let matcher = Matcher::Regex(Regex::new(re).map_err(|e| format!("invalid app-id regex: {e}"))?);
            decide(&windows, &matcher, mode, Vec::new())
        } else {
            decide_active_tab(&windows, mode)
        }
    } else {
        let (matcher, launch_cmd) = match &cli.app_id {
            Some(re) => {
                let m = Matcher::Regex(Regex::new(re).map_err(|e| format!("invalid app-id regex: {e}"))?);
                (m, cli.command.clone())
            }
            None => {
                if let Some((sub_name, modified)) = detect_terminal_subcommand(&cli.command) {
                    (Matcher::from_command(&sub_name), modified)
                } else {
                    (Matcher::from_command(&cli.command[0]), cli.command.clone())
                }
            }
        };
        decide(&windows, &matcher, mode, launch_cmd)
    };

    match decision {
        Decision::Launch(cmd) => {
            if cmd.is_empty() {
                return Ok(());
            }
            let err = Command::new(&cmd[0]).args(&cmd[1..]).exec();
            Err(match err.kind() {
                ErrorKind::NotFound => format!(
                    "cannot launch {:?}: command not found (PATH={})",
                    cmd[0],
                    std::env::var("PATH").unwrap_or_default()
                ),
                _ => format!("cannot launch {:?}: {err}", cmd[0]),
            })
        }
        Decision::FocusPrevious => action(&mut socket, Action::FocusWindowPrevious {}),
        Decision::Focus(id) => action(&mut socket, Action::FocusWindow { id }),
        Decision::Nothing => Ok(()),
    }
}

/// Report an error on stderr. niri discards the output of processes it spawns, so
/// when stderr isn't a terminal (i.e. we were started from a keybind) also show a
/// desktop notification. Otherwise a failed launch looks like an ignored key press.
fn report_error(msg: &str) {
    eprintln!("niri-appbeam: {msg}");
    if !std::io::stderr().is_terminal() {
        // Best effort: a missing notify-send or notification daemon is ignored.
        let _ = Command::new("notify-send")
            .args(["--app-name=niri-appbeam", "--urgency=critical", "niri-appbeam", msg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn main() -> ExitCode {
    match run(Cli::parse_from(normalize_args(std::env::args()))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            report_error(&e);
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

    fn launch(cmd: &[&str]) -> Vec<String> {
        cmd.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn launches_when_no_match() {
        let ws = [win(1, Some("ghostty"), true), win(2, None, false)];
        let cmd = launch(&["firefox"]);
        assert_eq!(decide(&ws, &ff(), Mode::Toggle, cmd.clone()), Decision::Launch(cmd.clone()));
        assert_eq!(decide(&ws, &ff(), Mode::Next, cmd.clone()), Decision::Launch(cmd.clone()));
        assert_eq!(decide(&ws, &ff(), Mode::Prev, cmd.clone()), Decision::Launch(cmd));
    }

    #[test]
    fn focuses_oldest_unfocused_match() {
        let ws = [
            win(9, Some("firefox"), false),
            win(3, Some("firefox"), false),
            win(1, Some("ghostty"), true),
        ];
        let cmd = launch(&["firefox"]);
        assert_eq!(decide(&ws, &ff(), Mode::Toggle, cmd.clone()), Decision::Focus(3));
        assert_eq!(decide(&ws, &ff(), Mode::Next, cmd.clone()), Decision::Focus(3));
        assert_eq!(decide(&ws, &ff(), Mode::Prev, cmd), Decision::Focus(3));
    }

    #[test]
    fn goes_back_when_already_focused() {
        let ws = [win(3, Some("firefox"), false), win(9, Some("firefox"), true)];
        assert_eq!(decide(&ws, &ff(), Mode::Toggle, launch(&["firefox"])), Decision::FocusPrevious);
    }

    #[test]
    fn tab_cycles_forward_and_wraps() {
        let at = |focused: u64| {
            [7, 2, 5, 1]
                .map(|id| win(id, Some(if id == 1 { "ghostty" } else { "firefox" }), id == focused))
        };
        assert_eq!(decide(&at(2), &ff(), Mode::Next, launch(&["firefox"])), Decision::Focus(5));
        assert_eq!(decide(&at(5), &ff(), Mode::Next, launch(&["firefox"])), Decision::Focus(7));
        assert_eq!(decide(&at(7), &ff(), Mode::Next, launch(&["firefox"])), Decision::Focus(2)); // wrap
    }

    #[test]
    fn tab_prev_cycles_backward_and_wraps() {
        let at = |focused: u64| [7, 2, 5].map(|id| win(id, Some("firefox"), id == focused));
        assert_eq!(decide(&at(7), &ff(), Mode::Prev, launch(&["firefox"])), Decision::Focus(5));
        assert_eq!(decide(&at(5), &ff(), Mode::Prev, launch(&["firefox"])), Decision::Focus(2));
        assert_eq!(decide(&at(2), &ff(), Mode::Prev, launch(&["firefox"])), Decision::Focus(7)); // wrap
    }

    #[test]
    fn tab_with_single_focused_window_does_nothing() {
        let ws = [win(4, Some("firefox"), true), win(1, Some("ghostty"), false)];
        assert_eq!(decide(&ws, &ff(), Mode::Next, launch(&["firefox"])), Decision::Nothing);
        assert_eq!(decide(&ws, &ff(), Mode::Prev, launch(&["firefox"])), Decision::Nothing);
    }

    #[test]
    fn active_tab_cycles_currently_focused_app() {
        let ws = [
            win(2, Some("firefox"), true),
            win(5, Some("ghostty"), false),
            win(8, Some("firefox"), false),
            win(12, Some("firefox"), false),
        ];
        // At window 2 -> next is 8
        assert_eq!(decide_active_tab(&ws, Mode::Next), Decision::Focus(8));
        // At window 2 -> prev wraps to 12
        assert_eq!(decide_active_tab(&ws, Mode::Prev), Decision::Focus(12));

        let ws_at_12 = [
            win(2, Some("firefox"), false),
            win(5, Some("ghostty"), false),
            win(8, Some("firefox"), false),
            win(12, Some("firefox"), true),
        ];
        // At window 12 -> next wraps to 2
        assert_eq!(decide_active_tab(&ws_at_12, Mode::Next), Decision::Focus(2));
    }

    #[test]
    fn active_tab_appends_new_windows_automatically() {
        // Earlier session had window 2 and 8
        // User opens a new window, which gets id 15 (appended by sort order)
        let ws = [
            win(15, Some("firefox"), false), // newly created window, even if returned earlier in list
            win(8, Some("firefox"), true),
            win(2, Some("firefox"), false),
        ];
        // At 8 -> next is 15
        assert_eq!(decide_active_tab(&ws, Mode::Next), Decision::Focus(15));
    }

    #[test]
    fn active_tab_single_window_does_nothing() {
        let ws = [win(2, Some("firefox"), true), win(5, Some("ghostty"), false)];
        assert_eq!(decide_active_tab(&ws, Mode::Next), Decision::Nothing);
        assert_eq!(decide_active_tab(&ws, Mode::Prev), Decision::Nothing);
    }

    #[test]
    fn active_tab_no_focused_window_does_nothing() {
        let ws = [win(2, Some("firefox"), false), win(5, Some("ghostty"), false)];
        assert_eq!(decide_active_tab(&ws, Mode::Next), Decision::Nothing);
    }

    #[test]
    fn detect_terminal_subcommand_ghostty() {
        let cmd = launch(&["ghostty", "-e", "yazi"]);
        let (sub, modified) = detect_terminal_subcommand(&cmd).unwrap();
        assert_eq!(sub, "yazi");
        assert_eq!(modified, launch(&["ghostty", "--class=appbeam.yazi", "-e", "yazi"]));

        let cmd_path = launch(&["/usr/bin/ghostty", "-e", "/usr/bin/herdr"]);
        let (sub, modified) = detect_terminal_subcommand(&cmd_path).unwrap();
        assert_eq!(sub, "herdr");
        assert_eq!(modified, launch(&["/usr/bin/ghostty", "--class=appbeam.herdr", "-e", "/usr/bin/herdr"]));

        // Existing --class is not overridden
        let cmd_existing = launch(&["ghostty", "--class=dev.yazi", "-e", "yazi"]);
        assert!(detect_terminal_subcommand(&cmd_existing).is_none());

        // Plain ghostty is not modified
        let cmd_plain = launch(&["ghostty"]);
        assert!(detect_terminal_subcommand(&cmd_plain).is_none());
    }

    #[test]
    fn detect_terminal_subcommand_foot_and_others() {
        let cmd_foot = launch(&["foot", "yazi"]);
        let (sub, modified) = detect_terminal_subcommand(&cmd_foot).unwrap();
        assert_eq!(sub, "yazi");
        assert_eq!(modified, launch(&["foot", "--app-id=appbeam.yazi", "yazi"]));

        let cmd_alacritty = launch(&["alacritty", "-e", "yazi"]);
        let (sub, modified) = detect_terminal_subcommand(&cmd_alacritty).unwrap();
        assert_eq!(sub, "yazi");
        assert_eq!(modified, launch(&["alacritty", "--class=appbeam.yazi", "-e", "yazi"]));

        let cmd_kitty = launch(&["kitty", "yazi"]);
        let (sub, modified) = detect_terminal_subcommand(&cmd_kitty).unwrap();
        assert_eq!(sub, "yazi");
        assert_eq!(modified, launch(&["kitty", "--class=appbeam.yazi", "yazi"]));
    }

    #[test]
    fn name_matcher_rules() {
        let m = |cmd: &str, id: &str| Matcher::from_command(cmd).matches(id);
        assert!(m("firefox", "firefox"));
        assert!(m("/usr/bin/firefox", "firefox"));
        assert!(m("obsidian", "md.obsidian.Obsidian"));
        assert!(m("ghostty", "com.mitchellh.ghostty"));
        assert!(m("yazi", "appbeam.yazi")); // auto-class matching!
        assert!(m("herdr", "appbeam.herdr"));
        assert!(m("yazi", "dev.yazi"));
        assert!(!m("ghostty", "appbeam.yazi")); // isolated!
        assert!(!m("yazi", "com.mitchellh.ghostty"));
        assert!(m("org.mozilla.firefox", "org.mozilla.firefox"));
        assert!(!m("firefox", "firefox-developer-edition"));
        assert!(!m("fire", "firefox"));
    }

    #[test]
    fn regex_matcher() {
        let m = Matcher::Regex(Regex::new(r"^dev\.yazi$").unwrap());
        assert!(m.matches("dev.yazi"));
        assert!(!m.matches("com.mitchellh.ghostty"));
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn single_dash_tab_spellings_are_accepted() {
        let n = |v: &[&str]| normalize_args(args(v));
        assert_eq!(n(&["nab", "-tab"]), args(&["nab", "--tab"]));
        assert_eq!(n(&["nab", "-tabprev"]), args(&["nab", "--tab-prev"]));
        assert_eq!(n(&["nab", "-tab", "firefox"]), args(&["nab", "--tab", "firefox"]));
        assert_eq!(n(&["nab", "-tabprev", "firefox"]), args(&["nab", "--tab-prev", "firefox"]));
        assert_eq!(n(&["nab", "-a", "-tab", "x"]), args(&["nab", "-a", "-tab", "x"]));
        assert_eq!(n(&["nab", "foo", "-tab"]), args(&["nab", "foo", "-tab"]));
        assert_eq!(n(&["nab", "--", "-tab"]), args(&["nab", "--", "-tab"]));
    }

    #[test]
    fn cli_parses_tab_flags_with_and_without_command() {
        let p = |v: &[&str]| Cli::try_parse_from(normalize_args(args(v)));
        // No command
        let c = p(&["nab", "-tab"]).unwrap();
        assert!(c.tab && !c.tab_prev);
        assert!(c.command.is_empty());

        let c = p(&["nab", "-tabprev"]).unwrap();
        assert!(c.tab_prev && !c.tab);
        assert!(c.command.is_empty());

        // With command
        let c = p(&["nab", "-tab", "firefox", "--new-window"]).unwrap();
        assert!(c.tab && !c.tab_prev);
        assert_eq!(c.command, args(&["firefox", "--new-window"]));

        let c = p(&["nab", "-tabprev", "-a", "^ff$", "firefox"]).unwrap();
        assert!(c.tab_prev && !c.tab);

        assert!(p(&["nab", "--tab", "--tab-prev"]).is_err());
    }
}
