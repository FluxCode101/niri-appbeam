# niri-appbeam

**Run-or-raise for the [niri](https://github.com/YaLTeR/niri) Wayland compositor.**

Bind one key per app. Each key press does the right thing:

| State                         | What happens                                   |
|-------------------------------|------------------------------------------------|
| App is not running            | **Launch** it                                  |
| App is open but not focused   | **Focus** it (switches workspace/monitor too)  |
| App is already focused        | **Jump back** to the previously focused window |

So `Hyper+B` takes you to the browser, and pressing `Hyper+B` again takes you back.

niri-appbeam talks to niri directly over its IPC socket (`$NIRI_SOCKET`), so it doesn't need
`jq`, `niri msg` or shell scripts. X11 tools like `wmctrl` and `xdotool` don't work on niri,
so you need something like this.

---

## Requirements

- [niri](https://github.com/YaLTeR/niri) **26.04** or newer
- Rust toolchain (`cargo`) to build, either from your distro or from [rustup](https://rustup.rs)

## Installation

```sh
git clone https://github.com/FluxCode101/niri-appbeam.git
cd niri-appbeam
make install
```

This builds a release binary and installs it to **`~/.local/bin/niri-appbeam`**. You don't need
root.

| Want                     | Command                                        |
|--------------------------|------------------------------------------------|
| Install for your user    | `make install`                                 |
| Install system-wide      | `sudo make install PREFIX=/usr/local`          |
| Custom location          | `make install PREFIX=/opt/niri-appbeam`        |
| Packaging (DESTDIR)      | `make install PREFIX=/usr DESTDIR="$pkgdir"`   |
| Uninstall                | `make uninstall` (same `PREFIX` as install)    |

<details>
<summary>Without make</summary>

```sh
cargo build --release --locked
install -Dm755 target/release/niri-appbeam ~/.local/bin/niri-appbeam
```

or, with cargo's own installer (installs to `~/.cargo/bin` unless you pass `--root`):

```sh
cargo install --locked --path . --root ~/.local
```

</details>

## Usage

```text
niri-appbeam [--app-id <REGEX>] <COMMAND> [ARGS...]
```

### The short form

For most apps the command name alone is enough:

```sh
niri-appbeam firefox
niri-appbeam obsidian
niri-appbeam ghostty
```

niri-appbeam compares the command name (case-insensitive) with each window's **app-id**. A
window matches if its whole app-id or the last dot-separated part of it equals the name:

| Command    | Matches app-id          |
|------------|-------------------------|
| `firefox`  | `firefox`               |
| `obsidian` | `md.obsidian.Obsidian`  |
| `ghostty`  | `com.mitchellh.ghostty` |

Matching is exact, so `firefox` does **not** match `firefox-developer-edition`.

### Explicit app-id

If the command name and the app-id are unrelated, pass a regex with `--app-id` (`-a`):

```sh
niri-appbeam --app-id '^org\.telegram\.desktop$' telegram-desktop
```

To find the app-id of a running window:

```sh
niri msg windows          # all windows
niri msg focused-window   # the one you're looking at
```

### Terminal apps (yazi, htop, ...)

TUI apps running inside a terminal share the **terminal's** app-id, so give each one its own
class:

```sh
niri-appbeam --app-id '^dev\.yazi$' ghostty --class=dev.yazi -e yazi
niri-appbeam --app-id '^dev\.htop$' foot --app-id=dev.htop htop
```

Everything after `<COMMAND>` is passed to it unchanged, flags included.

## niri configuration

Add binds to `~/.config/niri/config.kdl`. The config reloads automatically when you save it.

```kdl
binds {
    // "Hyper" = Mod+Alt+Ctrl+Shift
    Mod+Alt+Ctrl+Shift+B hotkey-overlay-title="Browser"  { spawn "niri-appbeam" "firefox"; }
    Mod+Alt+Ctrl+Shift+J hotkey-overlay-title="Notes"    { spawn "niri-appbeam" "obsidian"; }
    Mod+Alt+Ctrl+Shift+T hotkey-overlay-title="Terminal" { spawn "niri-appbeam" "ghostty"; }
    Mod+Alt+Ctrl+Shift+Y hotkey-overlay-title="Files"    {
        spawn "niri-appbeam" "--app-id" "^dev\\.yazi$" "ghostty" "--class=dev.yazi" "-e" "yazi";
    }
}
```

> **Note:** KDL strings need regex backslashes doubled: `"^dev\\.yazi$"`.

### "niri-appbeam: command not found" from niri

niri usually doesn't read your shell's `PATH`. It gets its environment from the systemd user
session, which often doesn't include `~/.local/bin`. Pick one:

1. **Use the full path** in binds (niri expands `~`):
   ```kdl
   Mod+Alt+Ctrl+Shift+B { spawn "~/.local/bin/niri-appbeam" "firefox"; }
   ```
2. **Add it to niri's environment** in `config.kdl`:
   ```kdl
   environment {
       PATH "/home/YOU/.local/bin:/usr/local/bin:/usr/bin:/bin"
   }
   ```
3. **Install system-wide:** `sudo make install PREFIX=/usr/local`.

Check what niri sees with `systemctl --user show-environment | grep ^PATH`.

## Behaviour details

- **Several windows of the same app:** the oldest one (lowest window id) gets focused.
- **Jump back** uses niri's built-in `focus-window-previous`. A window you only passed through
  for a split second may not count as "previous" (niri seems to need it focused for a moment).
- **Launching** replaces the niri-appbeam process with the app (`exec`), so nothing is left
  running in the background.
- **Exit codes:** `0` on success, `1` on errors (bad regex, niri unreachable, command not found).
  Errors are printed to stderr with the prefix `niri-appbeam:`.

## Development

```sh
make test     # unit tests
make build    # release build -> target/release/niri-appbeam
cargo clippy -- -D warnings
```

The decision logic (`decide`) is a pure function, separate from the IPC code, so it can be
unit-tested without a running niri.
