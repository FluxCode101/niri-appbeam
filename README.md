# niri-appbeam

**Run-or-raise-or-goto-previous for the [niri](https://github.com/YaLTeR/niri) Wayland compositor. Similar to Raycast App Switching on MacOS. Build in Rust.**


Example `Hyper+B` launches the browser if app is not open, or will tab to it, or if already active: pressing `Hyper+B` again takes you back to previous window.
`niri-appbeam` also allows tabbing between terminal applications out of the box
Simple, Minimal, no overhead, standalone, zero configuration needed, keep all configuration in your /.config/niri/config.kdl as usuall.

Just bind a key per app in "~/.config/niri/config.kdl". Each key press does the right thing:

| State                         | What happens                                   |
|-------------------------------|------------------------------------------------|
| App is not running            | **Launch** it                                  |
| App is open but not focused   | **Focus** it (switches workspace/monitor too)  |
| App is already focused        | **Jump back** to the previously focused window |

## How it works
niri-appbeam talks to niri directly over its IPC socket (`$NIRI_SOCKET`), so it doesn't need
`jq`, `niri msg` or shell scripts. X11 tools like `wmctrl` and `xdotool` don't work on niri,
so you need this specialized tool.

---

## Requirements

- [niri](https://github.com/YaLTeR/niri) **26.04** or newer
- Rust toolchain (`cargo`) to build, either from your distro or from [rustup](https://rustup.rs)

## Installation

```sh
git clone https://github.com/FluxCode101/niri-appbeam.git
cd niri-appbeam
make                # build as your normal user
sudo make install   # -> /usr/local/bin/niri-appbeam
```

This installs to **`/usr/local/bin`**. niri's `PATH` includes that folder on practically every
distro, so `spawn "niri-appbeam" ...` in your binds works without extra setup.

After every install, the installer looks up the `PATH` niri **really** uses and checks that
niri can find the binary. You get a warning if it can't, or if an older copy elsewhere comes
first. Run `make check-path` to repeat the check at any time.

> [!IMPORTANT]
> Build with `make` as your normal user. Use `sudo` **only** for `make install`. As root,
> `make install` doesn't build anything (cargo shouldn't run as root). If nothing is built
> yet, it asks you to run `make` first.

### Choosing where to install

| Want                         | Command                                      | Installs to             | niri finds it?            |
|------------------------------|----------------------------------------------|-------------------------|---------------------------|
| **System-wide** (recommended)| `make && sudo make install`                  | `/usr/local/bin`        | ✅ yes                    |
| Only for your user, no root  | `make install-user`                          | `~/.local/bin`          | ⚠️ often **not**, see below |
| Custom location              | `make install PREFIX=/opt/niri-appbeam`      | `/opt/niri-appbeam/bin` | ⚠️ only if on niri's PATH |
| Packaging (DESTDIR)          | `make install PREFIX=/usr DESTDIR="$pkgdir"` | `$pkgdir/usr/bin`       | n/a                       |
| Uninstall                    | `sudo make uninstall` / `make uninstall-user`| (same as install)       |                           |
| Which copy does niri run?    | `make check-path`                            |                         |                           |

> [!WARNING]
> **Why `~/.local/bin` is a trap:** niri doesn't use your shell's `PATH`. It inherits the
> environment of the systemd user session (or whatever started it), and that usually
> **doesn't** include `~/.local/bin`. `niri-appbeam firefox` then works in your terminal,
> but the same command in a niri bind **silently does nothing**. If you still want a user
> install, follow one of the fixes in
> [the troubleshooting section](#niri-appbeam-works-in-the-terminal-but-the-keybind-does-nothing).

<details>
<summary>Without make</summary>

```sh
cargo build --release --locked
sudo install -Dm755 target/release/niri-appbeam /usr/local/bin/niri-appbeam
```

Avoid `cargo install` here: it puts the binary in `~/.cargo/bin`, which niri usually doesn't
see either. That's the same trap as `~/.local/bin`.

</details>

## Usage

```text
niri-appbeam [--app-id <REGEX>] [--tab | --tab-prev] <COMMAND> [ARGS...]
```

### The short form

For most apps the command name alone is enough:

```sh
niri-appbeam firefox
niri-appbeam obsidian
niri-appbeam ghostty
```

### With parameter

```sh
```shell
niri-appbeam ghostty -e yazi
niri-appbeam ghostty -e herdr
```

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
niri-appbeam --app-id '^Spotify$' spotify-launcher   # Arch's spotify-launcher package
```

To find the app-id of a running window:

```sh
niri msg windows          # all windows
niri msg focused-window   # the one you're looking at
```

> [!TIP]
> The command must be something that actually exists on niri's `PATH`. Arch's
> `spotify-launcher` package has **no** `spotify` command. `niri-appbeam spotify` can still
> *raise* a running Spotify (app-id `Spotify`), but launching it fails. If niri-appbeam can't
> launch something, it now shows a desktop notification with the error (via `notify-send`)
> instead of failing silently.

### Cycling through windows of an app (`--tab`, `--tab-prev`)

Run without any command to cycle through all windows of the **currently active app**:

```sh
niri-appbeam --tab        # also accepted: -tab
niri-appbeam --tab-prev   # also accepted: -tabprev, --tabprev
```

- When you are in Firefox, pressing it cycles through all open Firefox windows.
- When you are in Ghostty, it cycles through your Ghostty windows.
- Windows are visited in the order they were opened (niri window id), and newly created windows are automatically appended to the cycle.
- If only one window of the active app is open, it does nothing (no unexpected jumps).

You can also pass an app name (`niri-appbeam --tab firefox`) to specifically focus/cycle that app:

| State                                  | `--tab` / `--tab-prev`                          |
|----------------------------------------|-------------------------------------------------|
| App is not running                     | **Launch** it                                   |
| App is open but not focused            | **Focus** its oldest window                     |
| One of its windows is focused          | Focus the **next** / **previous** window (wraps around) |
| Its only window is already focused     | Nothing happens                                 |

### Terminal apps (yazi, herdr, htop, ...)

`niri-appbeam` automatically detects when you run a command inside a terminal emulator (`ghostty`, `foot`, `alacritty`, `kitty`, `wezterm`):

```sh
niri-appbeam ghostty -e yazi
niri-appbeam ghostty -e herdr
niri-appbeam foot htop
```

Under the hood, `niri-appbeam`:
1. Injects an isolated window class on launch (e.g. `--class=appbeam.yazi` for Ghostty or `--app-id=appbeam.htop` for Foot).
2. Matches against that specific child app rather than the generic terminal.
3. Keeps your regular terminal windows (`niri-appbeam ghostty`) completely separate from your terminal apps!

No manual `--class` or `--app-id` flags are needed in your config. (You can still pass `--class` manually or use `--app-id` if you want a custom regex).

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
    Mod+Alt+Ctrl+Shift+S hotkey-overlay-title="Spotify"  {
        spawn "niri-appbeam" "--app-id" "^Spotify$" "spotify-launcher";
    }

    // Cycle through all Firefox windows, forward and backward
    Mod+Alt+Ctrl+Shift+Period hotkey-overlay-title="Next Firefox"  { spawn "niri-appbeam" "--tab" "firefox"; }
    Mod+Alt+Ctrl+Shift+Comma  hotkey-overlay-title="Prev Firefox"  { spawn "niri-appbeam" "--tab-prev" "firefox"; }
}
```

> **Note:** KDL strings need regex backslashes doubled: `"^dev\\.yazi$"`.

### niri-appbeam works in the terminal, but the keybind does nothing

niri **doesn't** use your shell's `PATH`. It inherits the environment of whatever started it,
usually the systemd user session, which often doesn't include `~/.local/bin` or
`~/.cargo/bin`. A bind that spawns a binary niri can't find fails **silently**.

First check which binary niri actually resolves (if any):

```sh
make check-path
```

Then pick one fix:

1. **Install system-wide** (most robust): `make uninstall-user && make && sudo make install`.
2. **Use the absolute path** in your binds:
   ```kdl
   Mod+Alt+Ctrl+Shift+B { spawn "/home/YOU/.local/bin/niri-appbeam" "firefox"; }
   ```
3. **Add the folder to your session `PATH`**, then log out and back in. This works when niri
   is started through `niri-session` or a display manager:
   ```sh
   mkdir -p ~/.config/environment.d
   echo 'PATH=$HOME/.local/bin:$PATH' >> ~/.config/environment.d/60-niri-appbeam.conf
   ```
4. **Override `PATH` for spawned processes** in `config.kdl`. This *replaces* the whole
   `PATH`, so list every folder you need:
   ```kdl
   environment {
       PATH "/home/YOU/.local/bin:/usr/local/bin:/usr/bin:/bin"
   }
   ```

You can inspect the raw environment yourself with
`systemctl --user show-environment | grep ^PATH` or
`tr '\0' '\n' < /proc/$(pgrep -xo niri)/environ | grep ^PATH`.

## Behaviour details

- **Several windows of the same app:** the oldest one (lowest window id) gets focused. With
  `--tab` / `--tab-prev`, further presses cycle through all of them in that order.
- **Jump back** uses niri's built-in `focus-window-previous`. A window you only passed through
  for a split second may not count as "previous" (niri seems to need it focused for a moment).
- **Launching** replaces the niri-appbeam process with the app (`exec`), so nothing is left
  running in the background.
- **Exit codes:** `0` on success, `1` on errors (bad regex, niri unreachable, command not found).
  Errors are printed to stderr with the prefix `niri-appbeam:`. niri throws away the output of
  the processes it spawns, so if stderr isn't a terminal (i.e. it was started from a keybind),
  the error is also shown as a desktop notification via `notify-send`, if that's installed.

## Development

```sh
make test     # unit tests
make build    # release build -> target/release/niri-appbeam
cargo clippy -- -D warnings
```

The decision logic (`decide`) is a pure function, separate from the IPC code, so it can be
unit-tested without a running niri.
