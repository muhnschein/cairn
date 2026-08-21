# Decisions

Documented, reversible defaults. Each entry says what was decided, what it
costs, and what would reopen it. Revisiting one is a normal PR, with evidence;
the burden of proof is on the change.

## D1 — Decompressors: pure Rust

`ruzstd` for Zstandard, `lzma-rs` for LZMA2/xz. No C bindings. The parser is
ours because it touches attacker-controlled bytes; handing those same bytes to
a C decompressor puts C back on that path.

**Cost:** both are slower than `zstd`/`xz2` and far less exercised. On large
archives this is the throughput ceiling, and the cluster cache exists partly to
hide it.

**Reopens it:** a benchmark against a real archive showing they are the
bottleneck. The answer would still not be C in-process by default, but a C
decoder behind a build flag CI does not enable.

## D2 — Namespaces: flat path space, resolved internally

The API has no namespace segment. `{path}` is resolved in two probes: the
archive's content namespace (`C`, or `A` in older archives), then — if the path
looks like `N/rest` — namespace `N` with that remainder.

That order lets an entry genuinely named `A/foo` in the content namespace win
over namespace `A` entry `foo`. Cross-namespace links in older archives
(`../I/logo.png`) still resolve, arriving as `I/logo.png`.

**Cost:** an entry in namespace `I` named `logo.png` is shadowed by a content
entry literally named `I/logo.png`. Rare, deterministic, documented in
`cairn-api(7)`.

**Rejected:** namespaces as a path segment. It leaks a format detail into the
interface for archives that are being phased out, and every client would have
to learn it.

## D3 — Auth failure is indistinguishable

When `auth_token` is set, authentication is checked **before routing**. Every
request without a valid token gets `401`, whatever it asked for. An
unauthenticated client cannot tell a missing archive from a present one, a
valid uuid from a malformed one, or a real route from a typo.

For authenticated requests, a missing archive and a missing entry are both
`not_found`: the difference tells a client nothing it can act on, and
collapsing it removes a probe for which archives exist.

**Cost:** a client debugging its own request gets no help until it
authenticates. The API is for programs, and the daemon's log says what
happened.

## D4 — Cluster cache: one global budget, LRU

One cache shared by every archive, keyed by (archive, cluster), bounded by
total decompressed bytes (`cluster_cache_bytes`, default 64M). A per-archive
budget would make memory scale with the number of archives.

- Uncompressed clusters never enter it; they are served from the mapping.
- A body larger than the whole budget is returned uncached rather than emptying
  the cache for one entry.
- Decoding happens **outside** the cache lock. Two workers racing on a cold
  cluster may decode it twice; holding the lock across decompression would
  serialize the pool.
- Eviction scans for the oldest slot. Slots are bounded by budget ÷ cluster
  size — tens, not thousands — so an intrusive list is not worth it.

The cache is why a client asking repeatedly for a tiny entry in a large cluster
pays for one decode; `request_rate` bounds how fast it can force new ones.

## D5 — Suggestions: byte-exact title prefix

`/suggest` binary-searches the archive's title pointer list and returns entries
whose stored title starts with `q`, byte for byte. No case folding, no
diacritic folding, no normalization.

The title list is sorted by the stored title's bytes, so any folding makes the
sort order and the query order disagree; a correct case-insensitive answer
needs a second index (cairn builds none) or a scan (which the amplification
bound forbids).

**Cost:** `q=rhino` does not match `Rhinoceros`. For a human typing into a
search box this is close to useless, and clients wanting that must build their
own index.

**Reopens it:** the same thing that reopens full-text search — a sidecar index.
Folding belongs there, not here.

## D6 — HTTP/1.1 only, hand-rolled

No HTTP/2, no HTTP crate. HTTP/2 means an HPACK state machine and stream
priority logic — a large parser surface facing a hostile client, for no benefit
to a local API. The HTTP/1.1 parser is a few hundred lines with a fuzz target
pointed at it. `HTTP/1.0` is refused with `505` rather than half-supported.

**Cost:** no multiplexing, one connection per concurrent request. The worker
pool is sized for that.

## D7 — Fixed worker pool, created before confinement

`max_connections` threads are created at startup, park at a gate, and are
released only after the sandbox is applied. No thread is created after
confinement. This is what keeps `clone` off the seccomp allowlist, and it makes
the connection ceiling a property of the process rather than a counter that has
to be right. Connections beyond the pool wait in the kernel's accept queue.

Thread startup makes syscalls the serving loop never makes (`prctl` to set the
thread name), so confinement landing mid-startup kills the process. Workers
therefore report readiness *after* they finish starting, and confinement waits
for all of them.

## D8 — mmap, and what happens when the file changes

Archives are mapped, not read through `pread64`, and the daemon does not try to
catch `SIGBUS`.

When an archive is truncated under a running daemon, a *load* from a lost page
faults with `SIGBUS` and the process dies; a *write* whose source pages were
lost fails with `EFAULT`, so the transfer is cut short and the daemon survives.
Both are covered by `make chaos`. Neither produces a complete answer with wrong
bytes, which is the property that matters.

**Consequence:** archives are immutable once opened, the archive directory
should be a read-only mount, and replacing an archive means restarting. The
units set `Restart=on-failure`. Whoever can truncate the file already has write
access to the data directory and is outside the threat model.

## D9 — No `Date`, no `Server`, no `ETag`

`Date` would need a clock and a formatter for no local benefit; there is
nothing to advertise in `Server`. Archives are immutable once opened, so a
client wanting caching can key on the archive uuid and the resolved path from
`X-Cairn-Path`.

**Reopens it:** a concrete client that needs conditional requests. `ETag` would
be cheap — the uuid plus the entry index — but nothing asks for it.

## D10 — Test archives are built by committed code

`crates/testutil` crafts ZIM archives programmatically, and the fuzz seed
corpus in `fuzz/seeds/` is generated from it by `zim-craft`. Routine tests need
no archive present; large real archives stay optional.

This keeps the repository text, keeps the corpus regenerable, and makes a new
hostile case a few lines of builder rather than a hex editor. `testutil` is
never a dependency of `cairnd` or `cairn`: writing ZIM files is a non-goal, and
the builder exists only so the parser has something to refuse.

The builder emits the modern title-index layout by default (D12), with
`legacy_title_index()` for the pre-6.1 one. Both are covered — a corpus
containing only what the parser already handles proves nothing.

## D11 — A third-party decoder may panic; the parser contains it

`lzma-rs` panics with an arithmetic overflow on an xz footer claiming a
backward size of `u32::MAX` (`decode/xz.rs:52`), reachable from a crafted
archive through `zimfmt::decompress`.

`zimfmt::decompress` catches the unwind and reports
`Error::Decompress("decoder panicked")`: `zimfmt` promises its callers a
`Result` for every input, and that promise cannot depend on a dependency never
panicking. Second layer: a worker catches a panic from anywhere in the serving
path and loses the connection, not the thread — workers cannot be replaced
after confinement (D7), so a panic that killed one would shrink the pool
permanently.

**Cost:** a panicking decoder leaks whatever it allocated and prints through
the default hook. That is noise, but it is the only signal that a dependency
mishandled an input, so the hook stays.

**Under the fuzzer:** `libfuzzer-sys` installs a hook that calls `abort()`
before unwinding, so a contained panic would still stop the run.
`zimfmt::decompress` silences that hook for the duration of the decoder call,
only under `cfg(fuzzing)`. A panic anywhere else still aborts and is still
reported. The interaction has its own test.

Upstream still panics. A unit test and `fuzz/seeds/archive/xz-crash.zim` pin
the regression, so a dependency bump that fixes or reintroduces it is
visible.

## D12 — The title ordering lives in an entry, not the header

The header field at offset 40 was historically a position, but current libzim
writes `setTitleIdxPos(offset_type(-1))` — a **sentinel** meaning "there is no
title index here". Treating it as a position overflows the region check and
rejects every archive Kiwix publishes. The real ordering is the entry
`X/listing/titleOrdered/v1`, whose blob libzim deliberately writes into an
*uncompressed* cluster so a reader can address it directly.

Resolve it the way libzim does, once at open:

1. `X/listing/titleOrdered/v1` if present and its cluster is uncompressed —
   front articles only, so usually shorter than the entry count;
2. otherwise the header's list, when the field is not the sentinel;
3. otherwise none, and `/suggest` says so.

Both cases reduce to an array of little-endian `u32` entry indices resident in
the mapped file, so `TitleIndex` names a position and a count and the search
code does not care which it got.

An archive with no ordering would otherwise make `/suggest` return an empty
list forever with no way to tell why, so `/v1/archives` carries
`"suggest": true|false` per archive.

Also mirrored from upstream: a header whose MIME table starts at 72 predates
the checksum field, so those bytes are table content rather than a position.

## D13 — The CLI reports; `--json` is the interface

`cairn` renders the daemon's answers for a person by default and prints the
JSON unchanged under `--json`, matching `clove(1)`. The reports are not an
interface and are not versioned; `--json` is, and it is the daemon's own answer
with nothing added. `get` and `head` are unaffected in both modes: entry
content is not JSON and is never reformatted.

Two things the reports do that the JSON cannot:

- `random` prints the path alone, so `cairn get "$uuid" "$(cairn random
  "$uuid")"` works without `jq`.
- an empty `suggest` result asks `/v1/archives/{uuid}` whether that archive has
  a title ordering at all (D12) and says which kind of empty it is. A daemon
  that will not answer counts as "it has one", so a failed second request never
  invents an explanation.

A failed command prints one line to stderr and leaves stdout empty. Under
`--json` the document is still on stdout, because that is what was asked for.

**Cost:** two output paths for five commands, and a JSON reader in a crate
whose manifest says it has no dependencies. The reader is small, is only ever
pointed at the local daemon, and is bounded.

## D14 — Archive text is scrubbed where it meets a terminal

Every string `cairn` prints in a report — titles, paths, MIME types, metadata
values — has control characters and the bidirectional overrides replaced with
`.`. Entry content written to a *terminal* is scrubbed too, keeping its own
newlines and tabs; content that is not text is refused there rather than left
to wedge the terminal.

A terminal is an interpreter and an archive is hostile input: `"\x1b[2J"`
clears the reader's screen, `"\x1b]0;…\x07"` retitles their window, and a bare
newline forges a table row. That is not a parser bug — `zimfmt` has no opinion
about `ESC` because `ESC` is not a format problem.

**Scrubbed at the boundary, not at the source.** The stored title is the
archive's actual title and `/v1/archives` keeps it; `api::json` escapes
everything below `0x20`, so `--json` consumers see what the archive really
says.

**Cost:** a title containing a tab renders with a `.` where the tab was.
Redirected or piped, `get` is exact — which is how anything is actually
extracted, and the reason scrubbing cannot corrupt a saved file.

## D15 — Opening a file is refused, not fatal

`open`, `openat` and `openat2` return `EACCES` instead of killing the process.
Everything else outside the allowlist still kills.

Nothing in the serving loop opens a file, but glibc's allocator does: it reads
`/proc/sys/vm/overcommit_memory` when growing a secondary arena's heap and
`/sys/devices/system/cpu/online` when counting processors for the arena limit,
the first time the workload needs it. Under concurrent requests that is
whenever the ninth worker allocates, which killed the daemon with `SIGSYS` on
real archives, intermittently.

**The security property is unchanged.** These calls were never allowed and
still are not: no path can be opened, ever, and the call cannot succeed — only
fail. What changes is that a library asking the filesystem an advisory question
gets an answer it already handles. Killing is right for a syscall that is
evidence; `openat` from glibc's malloc is housekeeping.

**Rejected:** adding `openat` to the allowlist (it is the one thing the
confinement rests on); pre-warming the allocator before confining (glibc
creates a secondary arena on *contention*, so forcing it at startup would make
the fix probabilistic); `mallopt(M_ARENA_MAX)` (measured — it does not stop the
`overcommit_memory` read, which is on the heap-growth path); a different
allocator (larger than everything in `DEPENDENCIES.md` put together).

`make sandbox` drives an allocation-heavy archive under concurrency, which is
what it takes to reach this at all. `/v1/status` reports `49 syscalls, 3
denied, kill on violation`, so the two classes are visible from outside.
