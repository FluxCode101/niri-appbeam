#!/bin/sh
# Check that niri will actually find (the right) niri-appbeam binary.
#
#   check-path.sh [BINDIR] [BIN]
#
# niri does not use your shell's PATH. It inherits the environment of whatever
# started it (usually the systemd user session), optionally overridden by an
# `environment { PATH "..." }` block in config.kdl. A binary that works in your
# terminal can therefore be invisible to `spawn "niri-appbeam"` and the bind
# silently does nothing.
#
# With BINDIR: verify that niri resolves BIN to BINDIR/BIN.
# Without BINDIR: just report which copy niri will run.
#
# Exit status: 0 = fine (or unknown), 1 = niri won't find it / runs another copy.

set -u

bindir=${1:-}
bindir=${bindir%/}
bin=${2:-niri-appbeam}

if [ -t 1 ]; then
    BOLD=$(printf '\033[1m'); YEL=$(printf '\033[33m'); GRN=$(printf '\033[32m'); RST=$(printf '\033[0m')
else
    BOLD=; YEL=; GRN=; RST=
fi

# Real user even under `sudo make install`.
uid=${SUDO_UID:-$(id -u)}
if [ -n "${SUDO_USER:-}" ]; then
    user_home=$(getent passwd "$SUDO_USER" | cut -d: -f6)
else
    user_home=$HOME
fi

# --- 1. Find the PATH niri uses for `spawn` ---------------------------------
niri_path=
source_desc=

# 1a. An explicit `environment { PATH "..." }` in the niri config wins.
config=${NIRI_CONFIG:-${XDG_CONFIG_HOME:-$user_home/.config}/niri/config.kdl}
if [ -r "$config" ]; then
    cfg_path=$(sed -n 's/^[[:space:]]*PATH[[:space:]]\{1,\}"\([^"]*\)".*/\1/p' "$config" | tail -n 1)
    if [ -n "$cfg_path" ]; then
        niri_path=$cfg_path
        source_desc="environment {} block in $config"
    fi
fi

# 1b. The running niri compositor's own environment.
if [ -z "$niri_path" ]; then
    pid=$(pgrep -xo -u "$uid" niri 2>/dev/null || true)
    if [ -n "$pid" ] && [ -r "/proc/$pid/environ" ]; then
        niri_path=$(tr '\0' '\n' < "/proc/$pid/environ" | sed -n 's/^PATH=//p')
        source_desc="the running niri (pid $pid)"
    fi
fi

# 1c. The systemd user session niri would be started from.
if [ -z "$niri_path" ] && [ "$(id -u)" != 0 ] && command -v systemctl >/dev/null 2>&1; then
    niri_path=$(systemctl --user show-environment 2>/dev/null | sed -n 's/^PATH=//p')
    [ -n "$niri_path" ] && source_desc="the systemd user session"
fi

if [ -z "$niri_path" ]; then
    echo "note: couldn't determine niri's PATH (is niri running?)."
    echo "      Run 'make check-path' from inside your niri session to verify."
    exit 0
fi

# --- 2. Resolve the binary the way niri's spawn would -----------------------
resolved=
old_ifs=$IFS
IFS=:
for d in $niri_path; do
    [ -n "$d" ] || continue
    if [ -f "$d/$bin" ] && [ -x "$d/$bin" ]; then
        resolved="${d%/}/$bin"
        break
    fi
done
IFS=$old_ifs

canon() { realpath -- "$1" 2>/dev/null || printf '%s\n' "$1"; }

print_fixes() {
    dir=$1
    echo
    echo "  Fix it with one of these:"
    echo
    echo "  1. Install system-wide instead (recommended, /usr/local/bin is on niri's PATH):"
    echo "       make && sudo make install"
    echo
    echo "  2. Use the full path in your niri binds:"
    echo "       spawn \"$dir/$bin\" \"firefox\";"
    echo
    echo "  3. Add $dir to your session PATH, then log out and back in:"
    echo "       mkdir -p ~/.config/environment.d"
    echo "       echo 'PATH=$dir:\$PATH' >> ~/.config/environment.d/60-$bin.conf"
    echo "     (works when niri is started via niri-session / a display manager)"
}

# --- 3. Report ---------------------------------------------------------------
if [ -z "$bindir" ]; then
    if [ -n "$resolved" ]; then
        echo "${GRN}ok:${RST} niri will run $BOLD$resolved$RST"
        echo "    (PATH from $source_desc)"
        exit 0
    fi
    echo "${YEL}${BOLD}warning:${RST} niri can't find '$bin' on its PATH."
    echo "    PATH from $source_desc:"
    echo "      $niri_path"
    for d in "$user_home/.local/bin" "$user_home/.cargo/bin"; do
        if [ -x "$d/$bin" ]; then
            echo "    It is installed in $d, which niri doesn't search."
            print_fixes "$d"
            break
        fi
    done
    exit 1
fi

target="$bindir/$bin"

if [ -n "$resolved" ] && [ "$(canon "$resolved")" = "$(canon "$target")" ]; then
    echo "${GRN}ok:${RST} niri will find $BOLD$target$RST - 'spawn \"$bin\"' works in your binds."
    exit 0
fi

echo
if [ -z "$resolved" ]; then
    echo "${YEL}${BOLD}warning: niri will NOT find $bin.${RST}"
    echo "  $bindir is not on niri's PATH, so 'spawn \"$bin\" ...' binds will silently do nothing"
    echo "  (even though running it from your terminal works)."
    echo
    echo "  PATH from $source_desc:"
    echo "    $niri_path"
    print_fixes "$bindir"
else
    echo "${YEL}${BOLD}warning: niri will run a DIFFERENT copy of $bin.${RST}"
    echo "  installed: $target"
    echo "  niri runs: $resolved   (comes first on niri's PATH)"
    echo
    echo "  That copy is probably stale. Remove it, e.g.:"
    case $resolved in
        "$user_home/.local/bin/$bin")  echo "    make uninstall-user" ;;
        "/usr/local/bin/$bin")         echo "    sudo make uninstall" ;;
        "$user_home"/*)                echo "    rm '$resolved'" ;;
        *)                             echo "    sudo rm '$resolved'" ;;
    esac
fi
echo
exit 1
