.POSIX:

CARGO ?= cargo
PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man
SYSCONFDIR ?= /etc
UNITDIR ?= $(PREFIX)/lib/systemd/system
USERUNITDIR ?= $(PREFIX)/lib/systemd/user
DESTDIR ?=
FUZZ_TIME ?= 60

.PHONY: all build test smoke chaos sandbox lint fmt man-lint doc-lint fuzz deps check install uninstall clean

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

doc-lint:
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps

# Both fuzz targets, seeded from the committed corpus. Needs a nightly
# toolchain and cargo-fuzz.
fuzz:
	mkdir -p fuzz/corpus/archive fuzz/corpus/request
	cp -n fuzz/seeds/archive/* fuzz/corpus/archive/ 2>/dev/null || true
	cp -n fuzz/seeds/request/* fuzz/corpus/request/ 2>/dev/null || true
	cd fuzz && $(CARGO) +nightly fuzz run archive -- -max_total_time=$(FUZZ_TIME)
	cd fuzz && $(CARGO) +nightly fuzz run request -- -max_total_time=$(FUZZ_TIME)

# Dependency allowlist, licenses, and the crate boundaries from the scope.
deps:
	ci/check-deps.sh
	ci/check-boundaries.sh

check: fmt lint test deps man-lint doc-lint

install: build
	install -d $(DESTDIR)$(BINDIR) $(DESTDIR)$(MANDIR)/man1 $(DESTDIR)$(MANDIR)/man5 \
		$(DESTDIR)$(MANDIR)/man7 $(DESTDIR)$(MANDIR)/man8 \
		$(DESTDIR)$(SYSCONFDIR)/cairn $(DESTDIR)$(UNITDIR) $(DESTDIR)$(USERUNITDIR)
	install -m 0755 target/release/cairnd $(DESTDIR)$(BINDIR)/cairnd
	install -m 0755 target/release/cairn $(DESTDIR)$(BINDIR)/cairn
	install -m 0644 man/cairn.1 $(DESTDIR)$(MANDIR)/man1/cairn.1
	install -m 0644 man/cairn.conf.5 $(DESTDIR)$(MANDIR)/man5/cairn.conf.5
	install -m 0644 man/cairn-api.7 $(DESTDIR)$(MANDIR)/man7/cairn-api.7
	install -m 0644 man/cairnd.8 $(DESTDIR)$(MANDIR)/man8/cairnd.8
	install -m 0644 -b contrib/cairn.conf $(DESTDIR)$(SYSCONFDIR)/cairn/cairn.conf
	sed 's|@BINDIR@|$(BINDIR)|g; s|@SYSCONFDIR@|$(SYSCONFDIR)|g' \
		systemd/cairnd.service > $(DESTDIR)$(UNITDIR)/cairnd.service
	sed 's|@BINDIR@|$(BINDIR)|g; s|@SYSCONFDIR@|$(SYSCONFDIR)|g' \
		systemd/cairnd-user.service > $(DESTDIR)$(USERUNITDIR)/cairnd.service

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/cairnd $(DESTDIR)$(BINDIR)/cairn
	rm -f $(DESTDIR)$(MANDIR)/man1/cairn.1 $(DESTDIR)$(MANDIR)/man5/cairn.conf.5
	rm -f $(DESTDIR)$(MANDIR)/man7/cairn-api.7 $(DESTDIR)$(MANDIR)/man8/cairnd.8
	rm -f $(DESTDIR)$(UNITDIR)/cairnd.service $(DESTDIR)$(USERUNITDIR)/cairnd.service

clean:
	$(CARGO) clean
	rm -rf fuzz/target fuzz/corpus fuzz/artifacts
