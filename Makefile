.POSIX:

CARGO ?= cargo
PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man
# Configuration follows the prefix, except on the two FHS system prefixes,
# where the daemon's compiled-in default (/etc/cairn/cairn.conf) and the man
# pages expect it. An explicitly given SYSCONFDIR always wins.
SYSCONFDIR ?= $(shell if test "$(PREFIX)" = /usr -o "$(PREFIX)" = /usr/local; \
	then echo /etc; else echo "$(PREFIX)/etc"; fi)
UNITDIR ?= $(PREFIX)/lib/systemd/system
USERUNITDIR ?= $(PREFIX)/lib/systemd/user
BASHCOMPDIR ?= $(PREFIX)/share/bash-completion/completions
ZSHCOMPDIR ?= $(PREFIX)/share/zsh/site-functions
FISHCOMPDIR ?= $(PREFIX)/share/fish/vendor_completions.d
DESTDIR ?=
FUZZ_TIME ?= 60

.PHONY: all build test smoke chaos sandbox lint fmt man-lint doc-lint completion-lint fuzz fuzz-seed deny deps check install uninstall clean

all: build

build:
	$(CARGO) build --release

# Unit, model, hostile-archive and hostile-request tests. No archive needed.
test:
	$(CARGO) test --workspace

# Daemon and CLI end to end over a crafted archive.
smoke:
	$(CARGO) test -p cairnd --test smoke

# Truncated files and archives replaced under a live daemon.
chaos:
	$(CARGO) test -p cairnd --test chaos

# The serving workload under the live seccomp filter.
sandbox:
	$(CARGO) test -p cairnd --test sandbox

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt:
	$(CARGO) fmt --all --check

man-lint:
	@if command -v mandoc >/dev/null 2>&1; then \
		mandoc -Tlint -Wwarning man/*.[1-8]; \
	else \
		echo "man-lint: mandoc not installed, skipping"; \
	fi

# Parse the completion scripts with whichever shells are installed. Best
# effort, like man-lint: a missing shell is a skipped check, not a failure.
completion-lint:
	@if command -v bash >/dev/null 2>&1; then bash -n completions/cairn.bash; \
	else echo "completion-lint: bash not installed, skipping"; fi
	@if command -v zsh >/dev/null 2>&1; then zsh -n completions/_cairn; \
	else echo "completion-lint: zsh not installed, skipping"; fi
	@if command -v fish >/dev/null 2>&1; then fish --no-execute completions/cairn.fish; \
	else echo "completion-lint: fish not installed, skipping"; fi

doc-lint:
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps

# Both fuzz targets, seeded from the committed corpus. Needs a nightly
# toolchain and cargo-fuzz. The corpus directory is kept: the nightly job
# restores it from the last run so coverage accumulates instead of restarting.
fuzz:
	mkdir -p fuzz/corpus/archive fuzz/corpus/request
	cp -n fuzz/seeds/archive/* fuzz/corpus/archive/ 2>/dev/null || true
	cp -n fuzz/seeds/request/* fuzz/corpus/request/ 2>/dev/null || true
	cd fuzz && $(CARGO) +nightly fuzz run archive -- -max_total_time=$(FUZZ_TIME)
	cd fuzz && $(CARGO) +nightly fuzz run request -- -max_total_time=$(FUZZ_TIME)

# Minimise the corpus and fold what it found into the committed seeds.
fuzz-seed:
	ci/fuzz-seed.sh

# Dependency allowlist, licenses, and the crate boundaries from the scope.
deps:
	ci/check-deps.sh
	ci/check-boundaries.sh

# Advisories, licences, banned crates and unknown sources. Needs cargo-deny.
deny:
	@if command -v cargo-deny >/dev/null 2>&1; then \
		$(CARGO) deny --all-features check; \
	else \
		echo "deny: cargo-deny not installed, skipping"; \
	fi

check: fmt lint test deps deny man-lint doc-lint completion-lint

install: build
	install -d $(DESTDIR)$(BINDIR) $(DESTDIR)$(MANDIR)/man1 $(DESTDIR)$(MANDIR)/man5 \
		$(DESTDIR)$(MANDIR)/man7 $(DESTDIR)$(MANDIR)/man8 \
		$(DESTDIR)$(SYSCONFDIR)/cairn $(DESTDIR)$(UNITDIR) $(DESTDIR)$(USERUNITDIR) \
		$(DESTDIR)$(BASHCOMPDIR) $(DESTDIR)$(ZSHCOMPDIR) $(DESTDIR)$(FISHCOMPDIR)
	install -m 0755 target/release/cairnd $(DESTDIR)$(BINDIR)/cairnd
	install -m 0755 target/release/cairn $(DESTDIR)$(BINDIR)/cairn
	install -m 0644 man/cairn.1 $(DESTDIR)$(MANDIR)/man1/cairn.1
	install -m 0644 man/cairn.conf.5 $(DESTDIR)$(MANDIR)/man5/cairn.conf.5
	install -m 0644 man/cairn-api.7 $(DESTDIR)$(MANDIR)/man7/cairn-api.7
	install -m 0644 man/cairnd.8 $(DESTDIR)$(MANDIR)/man8/cairnd.8
	install -m 0644 -b contrib/cairn.conf $(DESTDIR)$(SYSCONFDIR)/cairn/cairn.conf
	install -m 0644 completions/cairn.bash $(DESTDIR)$(BASHCOMPDIR)/cairn
	install -m 0644 completions/_cairn $(DESTDIR)$(ZSHCOMPDIR)/_cairn
	install -m 0644 completions/cairn.fish $(DESTDIR)$(FISHCOMPDIR)/cairn.fish
	sed 's|@BINDIR@|$(BINDIR)|g; s|@SYSCONFDIR@|$(SYSCONFDIR)|g' \
		systemd/cairnd.service > $(DESTDIR)$(UNITDIR)/cairnd.service
	sed 's|@BINDIR@|$(BINDIR)|g; s|@SYSCONFDIR@|$(SYSCONFDIR)|g' \
		systemd/cairnd-user.service > $(DESTDIR)$(USERUNITDIR)/cairnd.service
	@echo "install: to undo, run: make uninstall PREFIX=$(PREFIX) SYSCONFDIR=$(SYSCONFDIR)"
	@test -z "$(DESTDIR)" || echo "install: staged under DESTDIR=$(DESTDIR); remove it by hand"

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/cairnd $(DESTDIR)$(BINDIR)/cairn
	rm -f $(DESTDIR)$(MANDIR)/man1/cairn.1 $(DESTDIR)$(MANDIR)/man5/cairn.conf.5
	rm -f $(DESTDIR)$(MANDIR)/man7/cairn-api.7 $(DESTDIR)$(MANDIR)/man8/cairnd.8
	rm -f $(DESTDIR)$(UNITDIR)/cairnd.service $(DESTDIR)$(USERUNITDIR)/cairnd.service
	rm -f $(DESTDIR)$(BASHCOMPDIR)/cairn $(DESTDIR)$(ZSHCOMPDIR)/_cairn
	rm -f $(DESTDIR)$(FISHCOMPDIR)/cairn.fish
	rm -f $(DESTDIR)$(SYSCONFDIR)/cairn/cairn.conf~
	@if test -f "$(DESTDIR)$(SYSCONFDIR)/cairn/cairn.conf"; then \
		echo "uninstall: kept $(DESTDIR)$(SYSCONFDIR)/cairn/cairn.conf"; \
	fi

clean:
	$(CARGO) clean
	rm -rf fuzz/target fuzz/corpus fuzz/artifacts
