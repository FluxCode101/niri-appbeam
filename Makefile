# niri-appbeam — build & install helpers.
#
#   make                       build release binary
#   make install               install to ~/.local/bin   (no root needed)
#   make install PREFIX=/usr/local   (system-wide, run with sudo)
#   make uninstall [PREFIX=...]
#
# DESTDIR is supported for packaging (e.g. AUR PKGBUILDs):
#   make install PREFIX=/usr DESTDIR="$pkgdir"

BIN     := niri-appbeam
PREFIX  ?= $(HOME)/.local
BINDIR  ?= $(PREFIX)/bin
CARGO   ?= cargo

.PHONY: all build install uninstall test clean

all: build

build:
	$(CARGO) build --release --locked

install: build
	install -Dm755 target/release/$(BIN) "$(DESTDIR)$(BINDIR)/$(BIN)"
	@echo "Installed $(DESTDIR)$(BINDIR)/$(BIN)"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(BIN)"
	@echo "Removed $(DESTDIR)$(BINDIR)/$(BIN)"

test:
	$(CARGO) test --locked

clean:
	$(CARGO) clean
