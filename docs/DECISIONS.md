# Decisions

The open questions from §8 of the scope, resolved. Each records what was
decided, what it costs, and what would reopen it. A decision is not a
justification: where the choice is uncomfortable, the entry says so.

---

## D1 — Decompressor crates: pure Rust

**Decided:** `ruzstd` for Zstandard, `lzma-rs` for LZMA2/xz. No C bindings.

The parser is ours precisely because it touches attacker-controlled bytes
(§3.6). Handing those same bytes to a C decompressor puts C back on exactly the
path that argument was made about. `ruzstd` and `lzma-rs` are pure Rust, keep
the memory-safety story whole, and keep the tree auditable: seven crates, all
readable.

**Cost, stated plainly:** both are slower than `zstd`/`xz2`, and far less
battle-tested. On large archives this is the throughput ceiling, and the cluster
cache exists partly to hide it.

**What reopens it:** a benchmark against a real Wikipedia archive showing the
pure-Rust decoders are the bottleneck for a real deployment. If they are, the
answer is still not C in-process by default; it is a measured argument recorded
here, with the C decoder behind a build flag that CI does not enable.

---

## D2 — Namespaces: flat path space, resolved internally

**Decided:** the API has no namespace segment. `{path}` is resolved in two
probes:

1. the archive's content namespace (`C` in archives using the modern scheme,
   `A` in older ones);
2. failing that, if the path looks like `N/rest` where `N` is a single
   namespace character, that namespace with that remainder.

**Why this order:** an entry genuinely named `A/foo` in the content namespace
must win over namespace `A` entry `foo`. Cross-namespace links in older
archives (`../I/logo.png`) still resolve, because they arrive as `I/logo.png`
after the client resolves the relative reference.

**Cost:** an entry in namespace `I` named `logo.png` is shadowed by a content
entry literally named `I/logo.png`. Rare, deterministic, and documented in
`cairn-api(7)`.

**Rejected:** exposing namespaces as a path segment. It leaks a format detail
into the interface for the benefit of archives that are being phased out, and
every client would have to learn it.

---

## D3 — Auth failure is indistinguishable, by construction

**Decided:** when `auth_token` is set, authentication is checked **before
routing**. Every request without a valid token gets `401`, whatever it asked
for. An unauthenticated client cannot tell a missing archive from a present
one, a valid uuid from a malformed one, or a real route from a typo.

**Cost:** a client debugging its own request gets no help until it authenticates
correctly. Acceptable: the API is for programs, and the daemon's own log says
what happened.

**Related:** for *authenticated* requests, a missing archive and a missing entry
are both `not_found`. The difference tells a client nothing it can act on, and
collapsing it removes a probe for which archives exist.

---

## D4 — Cluster cache: one global budget, LRU

**Decided:** a single cache shared by every archive, keyed by (archive,
cluster), bounded by total decompressed bytes (`cluster_cache_bytes`, default
64M), evicting least-recently-used.

**Why global:** a per-archive budget makes memory scale with the number of
archives, which is exactly what an appliance with forty archives cannot afford.

**Details that matter:**

- Uncompressed clusters never enter the cache. They are served straight from
  the mapping, so caching them would copy bytes the kernel already has.
- A decoded body larger than the whole budget is returned uncached rather than
  emptying the cache for one entry.
- Decoding happens **outside** the cache lock. Two workers racing on the same
  cold cluster may decode it twice; a lock held across decompression would
  serialize every worker in the pool. Duplicated work is the cheaper mistake.
- Eviction scans for the oldest slot. The slot count is bounded by
  budget ÷ cluster size — tens, not thousands — so the scan is not worth an
  intrusive list.

**Interaction with amplification (§7.2):** the cache is the reason a client
asking repeatedly for a tiny entry in a large cluster pays for one decode, and
`request_rate` is the ceiling on how fast it can try to force new ones.

---

## D5 — Suggestions: byte-exact title prefix

**Decided:** `/suggest` binary-searches the archive's title pointer list and
returns entries whose stored title starts with `q`, byte for byte. No case
folding, no diacritic folding, no normalization.

**Cost, stated plainly:** `q=rhino` does not match `Rhinoceros`. For a human
typing into a search box this is close to useless, and clients that want that
behaviour must build their own index.

**Why anyway:** the title list is sorted by the stored title's bytes. Any
folding makes the sort order and the query order disagree, so a correct
case-insensitive answer needs either a second index (which cairn does not
build) or a scan (which §7.2 forbids). Plain prefix is honest about what the
archive actually offers.

**What reopens it:** the same thing that reopens full-text search — a sidecar
index. Then folding belongs there, not here.

---

## D6 — HTTP/1.1 only, hand-rolled

**Decided:** the request parser is written here, HTTP/1.1 only, no HTTP/2, no
HTTP crate.

HTTP/2 means an HPACK state machine and stream priority logic — a large parser
surface facing a hostile client, for no benefit to a local API. The HTTP/1.1
parser is a few hundred lines with a fuzz target pointed at it, and every bound
is explicit. `HTTP/1.0` is refused with `505` rather than half-supported.

**Cost:** no multiplexing, and one connection per concurrent request. The
worker pool is sized for that.

---

## D7 — Fixed worker pool, created before confinement

**Decided:** `max_connections` threads are created at startup, park at a gate,
and are released only after the sandbox is applied. No thread is created after
confinement.

This is what keeps `clone` off the seccomp allowlist, and it makes the
connection ceiling a property of the process rather than a counter that has to
be right. Connections beyond the pool wait in the kernel's accept queue.

**A race this exposed:** confinement landing while a worker is still starting
up kills the process, because thread startup makes syscalls (`prctl` to set the
thread name) that the serving loop never makes. Workers therefore report
readiness *after* they finish starting, and confinement waits for all of them.
Found by `make sandbox`, which is the point of having it.

---

## D8 — mmap, and what happens when the file changes

**Decided:** archives are mapped, not read through `pread64`, and the daemon
does not try to catch `SIGBUS`.

**What actually happens** when an archive is truncated under a running daemon:
a *load* from a lost page faults with `SIGBUS` and the process dies; a *write*
whose source pages were lost fails with `EFAULT` instead, so the transfer is cut
short and the daemon survives. Both are covered by `make chaos`. Neither
produces a complete answer with wrong bytes, which is the property that
matters.

**Consequence:** archives are immutable once opened, the archive directory
should be a read-only mount, and replacing an archive means restarting. The
units set `Restart=on-failure`. This is an availability property; whoever can
truncate the file already has write access to the data directory and is outside
the threat model.

---

## D9 — No `Date`, no `Server`, no `ETag`

**Decided:** responses carry no `Date` header (it would need a clock and a
formatter for no local benefit), no `Server` header (nothing to advertise), and
no `ETag`. Archives are immutable once opened, so a client that wants caching
can key on the archive uuid and the resolved path from `X-Cairn-Path`.

**What reopens it:** a concrete client that needs conditional requests. `ETag`
would be cheap — the uuid plus the entry index — but nothing asks for it yet.

---

## D10 — Test archives are built by committed code

**Decided:** `crates/testutil` crafts ZIM archives programmatically, and the
fuzz seed corpus in `fuzz/seeds/` is generated from it by `zim-craft`. Routine
tests need no archive present; large real archives stay optional.

This keeps the repository text, keeps the corpus regenerable, and makes a new
hostile case a few lines of builder rather than a hex editor. `testutil` is
never a dependency of `cairnd` or `cairn` — writing ZIM files is a non-goal
(§4), and the builder exists only so the parser has something to refuse.

---

## D11 — A third-party decoder may panic; the parser contains it

**Found by fuzz target A, within a minute of its first CI run:** `lzma-rs`
panics with an arithmetic overflow on an xz footer claiming a backward size of
`u32::MAX` (`decode/xz.rs:52`). A crafted archive reaches it through
`zimfmt::decompress`.

**Decided:** `zimfmt::decompress` catches the unwind and reports
`Error::Decompress("decoder panicked")`. `zimfmt` promises its callers a
`Result` for every input, and that promise cannot depend on a dependency never
panicking.

**Second layer:** a worker catches a panic from anywhere in the serving path
and loses the connection, not the thread. Workers cannot be replaced after
confinement (D7), so a panic that killed one would shrink the pool
permanently — a denial of service from a single crafted archive.

**Cost:** a panicking decoder leaves whatever it allocated to be dropped, and
prints its message to stderr through the default hook. That is noise, but it
is also the only signal that a dependency mishandled an input, so the hook
stays.

**The harness makes this awkward, and that is handled explicitly.**
`libfuzzer-sys` installs a panic hook that calls `abort()` *before* unwinding,
deliberately, so the fuzzer can report an intact stack. That hook runs before
`catch_unwind` ever gets a chance, so under the fuzzer a contained panic still
killed the run — the fuzz job stopped at this one defect instead of exploring
past it. `zimfmt::decompress` therefore silences the hook for the duration of
the decoder call, and only under `cfg(fuzzing)`, which cargo-fuzz sets for the
whole build. In a real daemon the default hook stays installed and the panic
message is printed before the unwind is caught; under the fuzzer, a panic
anywhere outside that one call still aborts and is still reported. The
interaction has its own test, which aborts if the mechanism is removed.

**Not done:** upstream still panics. The regression is pinned here by a unit
test and by `fuzz/seeds/archive/xz-crash.zim` in the seed corpus, so a
dependency bump that fixes or reintroduces it is visible.
