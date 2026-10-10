# niri-appbeam — build & install helpers.
#
#   make                         build release binary (as your normal user)
#   sudo make install            install to /usr/local/bin   (recommended)
#   make install-user            install to ~/.local/bin     (no root, see note)
#   make install PREFIX=/opt/x   custom location
#   make uninstall / make uninstall-user
#   make check-path              show which niri-appbeam niri will actually run
#
# Why system-wide is the default: niri does NOT read your shell's PATH. It
# inherits the systemd user session's environment, which on most distros does
# not include ~/.local/bin. A user install then works in your terminal but
# `spawn "niri-appbeam"` binds silently do nothing. After every install the
# PATH niri really uses is checked and you get a warning if it can't find the
# binary.
#
# DESTDIR is supported for packaging (e.g. AUR PKGBUILDs):
#   make install PREFIX=/usr DESTDIR="$pkgdir"

BIN         := niri-appbeam
PREFIX      ?= /usr/local
BINDIR      ?= $(PREFIX)/bin
USER_PREFIX ?= $(HOME)/.local
CARGO       ?= cargo
TARGET      := target/release/$(BIN)

.PHONY: all build install install-user uninstall uninstall-user check-path test clean

all: build

build:
	$(CARGO) build --release --locked

install:
	@set -e; \
	if [ "$$(id -u)" = 0 ]; then \
		if [ ! -x "$(TARGET)" ]; then \
			echo "error: $(TARGET) is not built yet." >&2; \
			echo "       Build as your normal user first (cargo shouldn't run as root):" >&2; \
			echo "         make && sudo make install" >&2; \
			exit 1; \
		fi; \
	else \
		$(CARGO) build --release --locked; \
	fi; \
	if [ -z "$(DESTDIR)" ]; then \
		d="$(BINDIR)"; while [ ! -d "$$d" ]; do d=$$(dirname "$$d"); done; \
		if [ ! -w "$$d" ]; then \
			echo "" >&2; \
			echo "error: no write permission for $(BINDIR)." >&2; \
			echo "       The binary is built, now install it with one of:" >&2; \
			echo "         sudo make install     -> /usr/local/bin  (recommended, niri finds it)" >&2; \
			echo "         make install-user     -> ~/.local/bin    (no root, may need extra setup)" >&2; \
			exit 1; \
		fi; \
	fi
	install -Dm755 "$(TARGET)" "$(DESTDIR)$(BINDIR)/$(BIN)"
	@echo "Installed $(DESTDIR)$(BINDIR)/$(BIN)"
	@if [ -z "$(DESTDIR)" ]; then sh scripts/check-path.sh "$(BINDIR)" "$(BIN)" || true; fi

install-user:
	@if [ "$$(id -u)" = 0 ] && [ -z "$(DESTDIR)" ]; then \
		echo "error: don't run install-user with sudo, it would install into root's home." >&2; \
		echo "       Use 'make install-user' as yourself, or 'sudo make install' for system-wide." >&2; \
		exit 1; \
	fi
	@$(MAKE) --no-print-directory install PREFIX="$(USER_PREFIX)"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(BIN)"
	@echo "Removed $(DESTDIR)$(BINDIR)/$(BIN)"

uninstall-user:
	@$(MAKE) --no-print-directory uninstall PREFIX="$(USER_PREFIX)"

check-path:
	@sh scripts/check-path.sh "" "$(BIN)"

test:
	$(CARGO) test --locked

clean:
	$(CARGO) clean
