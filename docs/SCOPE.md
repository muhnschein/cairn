# cairn — Scope

> **Status:** implemented through M5 (§10); pre-alpha and unaudited. This
> document defines what the project is, what it is deliberately not, and what
> "done" means for a release. It is the document later decisions are checked
> against. Where reality has since refined a claim here, the refinement is in
> [`DECISIONS.md`](DECISIONS.md) and this document points at it.

## §0 Name

**cairn** — a stack of stones raised so that whoever comes later can find the
way without asking anyone. Built once, read many times, needs no network to do
its job. The daemon is `cairnd(8)`; the control CLI is `cairn(1)`.

## §1 Overview

cairn serves the contents of ZIM archives over a small local HTTP API. It
returns stored entries and metadata. It does not render a website, rewrite
HTML, inject a reader, or present a catalogue page.

The intended consumers are other programs: a local reader UI written by someone
else, a search tool, a script, an offline agent. A human with `curl` is a
supported consumer; a human with a browser and no client is not the target.

ZIM is the format used by the openZIM project and consumed by Kiwix. The
reference server, `kiwix-serve`, is a full web application built on the C++
`libzim`. cairn is not a reimplementation of it and does not aim for feature
parity. It aims to be the smallest correct thing that can hand out bytes from a
ZIM archive while trusting neither the archive nor the client.

## §2 Platform

cairn targets a modern Linux kernel (6.12), `seccomp`, Landlock, and systemd,
on `x86_64`, `aarch64`, and `riscv64`. Two of its confinement layers are built
on kernel facilities that exist nowhere else. No effort is made to accommodate
other platforms, and portability is not accepted as a reason to weaken
confinement.

`sandbox require` in `cairn.conf` turns a partially-applied sandbox into a
refusal to start.

Rust toolchain is pinned in `rust-toolchain.toml`. Builds are reproducible from
a committed `Cargo.lock`.

## §3 Goals

1. **Two hostile inputs, not one.** Both the archive and the client are
   untrusted. A ZIM downloaded from a mirror is untrusted the way a torrent is;
   a request arriving on the socket is untrusted the way a peer message is.
   Neither is more privileged than the other, and both get a fuzz target.
2. **Read-only by construction.** cairn never writes to an archive, and holds
   no write capability to any path once initialization is over. No upload
   endpoint, no archive management, no mutation anywhere in the API surface.
3. **A small API, fully specified.** Every endpoint documented in
   `cairn-api(7)` before it is implemented. No undocumented endpoints, no debug
   routes that exist only in development builds.
4. **A reviewed dependency set.** Every crate in `DEPENDENCIES.md` with a
   reason. CI fails when a dependency is added without review. Particular
   scrutiny for anything that can open a socket, spawn a process, or touch the
   filesystem outside the archive directory.
5. **Confinement that is verifiable from outside.** `cairn status` and
   `/v1/status` report what was actually applied, because a daemon that failed
   to confine itself otherwise looks identical to one that succeeded.
6. **Own the parser.** No FFI to `libzim`, and no adoption of an existing ZIM
   crate. The parser touches attacker-controlled bytes; it is written here,
   from the openZIM specification, small enough to review in one sitting.

## §4 Non-goals

Permanent, not "not yet".

- **No web UI — ever.** No HTML rendering, no CSS, no bundled JavaScript, no
  catalogue page, no viewer.
- **No HTML rewriting.** Links inside archived pages are returned exactly as
  stored. cairn does not rewrite `href`s, strip scripts, or modify entry
  content. Anything else would put an HTML parser in the hot path. See §7.3 for
  what this means for browser-based clients.
- **No OPDS — ever.** No catalogue protocol of any kind. The archive list in
  §6 is the entire discovery surface, and it is not a feed.
- **No writing ZIM files.** cairn does not create, edit, or repair archives.
  `crates/testutil` writes them for tests only and is never a dependency of
  `cairnd` or `cairn` (D10).
- **No scraping.** cairn fetches nothing from the network and has no outbound
  socket capability at all.
- **No user accounts, sessions, or authorization roles.** Authentication, if
  present, is a single shared bearer token or nothing.
- **No TLS termination.** cairn listens on a unix socket or loopback. TLS is
  the reverse proxy's job.
- **No clustering, replication, or multi-node anything.**
- **No plugin system.**

**Full-text search is deferred, not refused.** cairn ships without it and
offers title-prefix suggestion only. This is a real capability gap versus
`kiwix-serve` and the README says so plainly. Revisiting it means either a
Xapian reader in Rust or a sidecar index, and both are their own project; see
§8.5 and D5.

## §5 Architecture

A Cargo workspace under `crates/`, with a hard boundary between parsing and
serving.

| Crate | Responsibility |
|---|---|
| `zimfmt` | Pure parser. Header, MIME table, pointer lists, dirents, clusters, decompression. No I/O policy, no HTTP, no logging of content. Fuzz target A. |
| `archive` | Opening and holding archives: mmap lifetime, UUID identity, redirect resolution, lookup by path and by title, bounds checks above the raw parser. |
| `api` | HTTP surface. Request parsing, routing, response construction, range handling, limits, error mapping. Knows nothing about ZIM internals beyond an entry handle. Fuzz target B. |
| `sandbox` | Landlock ruleset construction, seccomp filter construction, reporting of what was applied. No dependency on the rest of the workspace. |
| `cairnd` | Daemon. Config parsing, init ordering, socket setup, then confinement, then serve. |
| `cairn` | Control CLI. Speaks the same HTTP API over the local socket. |

`zimfmt` has no dependency that can perform I/O. `api` has no dependencies at
all. `cairn` has none either. These boundaries are enforced by
`ci/check-boundaries.sh` and by the `disallowed-types` list in `clippy.toml`,
not by convention, in the manner of clove's `ci/check-net-deps.sh`.

### 5.1 Parser

The ZIM format is a fixed-size header followed by a MIME type table, a URL
pointer list, a title pointer list, a cluster pointer list, and the clusters
themselves; entries may be redirects to another entry index. Clusters are
compressed — Zstandard by default in archives produced since 2021, with LZMA2
also in circulation, plus uncompressed clusters. **The openZIM specification is
the authority**; where this document and the specification disagree, the
specification wins and this document is wrong.

Written from the specification, not adapted from an existing implementation.
The `zim` crate (Apache-2.0/MIT) may be consulted as a reference where the
specification is ambiguous; no GPL-licensed source may be consulted or adapted,
for the reason in §11.

Every offset read from the file is validated against the file length before
use. Every cluster index is validated against the cluster count. Redirect
chains are followed to a fixed small depth and then abandoned. Decompressed
cluster size is bounded by configuration with a documented default, so a
crafted cluster cannot exhaust memory.

Two places where the specification and current libzim disagree with the older
written format are recorded in D12: the title-ordering sentinel at header
offset 40, and a MIME table starting at 72 in archives predating the checksum
field.

### 5.2 mmap and SIGBUS

Archives are mapped, not read through `pread64`. This is the right call for
multi-gigabyte files and matches every other implementation.

The consequence is accepted explicitly: if an archive is truncated or replaced
in place under a running daemon, a *load* from a lost page faults with
`SIGBUS`, which is not a recoverable error in safe Rust, and cairn does not
attempt to catch it. A *write* whose source pages were lost fails with
`EFAULT` instead, cutting the transfer short with the daemon alive. Neither
produces a complete answer with wrong bytes, which is the property that
matters; see D8.

This is documented in `cairnd(8)` as a deployment constraint — archives are
immutable once opened, the archive directory should be a read-only mount, and
replacing an archive requires a restart. The systemd units set
`Restart=on-failure`. `make chaos` covers both failure modes so they are known
and reproducible rather than discovered in the field.

This is an availability property, not a confidentiality or integrity one. An
attacker who can truncate the archive file already has write access to the data
directory and is outside the threat model.

### 5.3 Archive identity

Archives are addressed by the UUID stored in the archive header. Ids are
therefore stable across restarts, across renames, and across machines, and two
copies of the same archive are the same id by construction.

Consequences accepted: a duplicate UUID (same archive present twice, or a
crafted collision) is a startup error naming both paths, not a silent
preference; a client cannot guess an id from a filename, so `/v1/archives` is
the only way in; and an archive without a valid UUID is refused at open time.

## §6 API

Versioned under `/v1`. JSON for metadata, raw bytes for content.
`cairn-api(7)` is the specification; this is the shape.

```
GET  /v1/status                         daemon state, sandbox actually applied
GET  /v1/archives                       open archives: uuid, title, entry count
GET  /v1/archives/{uuid}                archive metadata from the M namespace
GET  /v1/archives/{uuid}/entry/{path}   entry content; Range supported
HEAD /v1/archives/{uuid}/entry/{path}   size and type without the body
GET  /v1/archives/{uuid}/random         one random entry path
GET  /v1/archives/{uuid}/suggest?q=     title-prefix suggestions
```

`{uuid}` is the canonical lowercase hyphenated form; any other form is a 400,
not a normalization. `{path}` is a flat path space with the namespace resolved
internally (D2).

Content responses carry `X-Cairn-Archive` and `X-Cairn-Path`, the resolved path
after redirect following, so a client can distinguish a redirect target from a
direct hit without a second request.

Errors are a single documented JSON shape that never reflects client input back
in the message. A malformed archive region produces `503` for that archive, not
a `500` for the daemon, and never a panic.

## §7 Security model

Three independent layers. No layer assumes another is present.

### 7.1 By construction

No component can open an outbound socket; the dependency check in CI fails if a
socket-capable crate reaches anything but the listener. No component writes to
the archive directory. No process is spawned. No DNS. The parser is written
here in Rust, with `unsafe` confined to the mmap boundary and the sandbox
syscalls, justified in a comment at each site.

### 7.2 The client is hostile

The request path gets the same treatment as the archive path. Specifically:

- **Request parsing is a fuzz target.** Raw bytes in, no assumption that a
  well-formed request is the common case. HTTP/1.1 only — no HTTP/2, so no
  HPACK state machine and no stream priority logic (D6). No request body is
  accepted on any endpoint; a non-zero `Content-Length` is refused before
  routing.
- **Header-injection through archive data.** MIME types come from the archive's
  own table and are attacker-controlled, and they end up in a response header.
  Every value written into a header is validated against a strict token
  grammar; anything containing CR, LF, NUL, or a non-token byte becomes
  `application/octet-stream`. This is the sharpest edge where the two hostile
  inputs meet, and it gets its own test module.
- **Path handling.** ZIM paths are keys into a table, not filesystem paths, so
  traversal cannot escape — but decoding must still be canonical: percent-decode
  exactly once, reject `%00`, reject over-long UTF-8, and never re-decode a
  decoded result.
- **Range requests.** Single range only. Multipart `byteranges` is refused
  outright: it is an amplification vector and a parser surface for no benefit
  to the intended clients. Unsatisfiable ranges are 416.
- **Amplification.** A tiny entry inside a large compressed cluster lets a
  client force repeated decompression cheaply. A bounded cluster cache plus a
  documented per-connection request rate ceiling; both configurable, both with
  defaults chosen against a real archive. Cache policy is D4.
- **Connection limits.** Maximum concurrent connections, maximum request line
  and header sizes, maximum header count, read and write timeouts, bounded
  keep-alive. All configurable, all with documented defaults, none unbounded.
  The connection ceiling is the size of a pool created before confinement (D7),
  not a counter.
- **Suggestion queries.** `q` is length-bounded and result-count-bounded. The
  lookup is a binary search over the title pointer list — no regex, no
  unbounded scan, no user-controlled iteration count.
- **Auth.** If a bearer token is configured, comparison is constant-time, and
  the check happens before routing so an unauthenticated request cannot
  distinguish "no such archive" from "not authorized" (D3).

### 7.3 What "no HTML rewriting" costs the client

Because cairn returns archived HTML unmodified, any browser-based client that
renders an entry is rendering attacker-controlled markup from the same origin
as the API. cairn will not solve this by rewriting, and cannot solve it fully
from the server side. It does what it can without parsing content:
`X-Content-Type-Options: nosniff`, `Cross-Origin-Resource-Policy: same-origin`,
and a `Content-Security-Policy` on content responses. `cairn-api(7)` states
plainly that clients rendering entries must do so in an isolated origin, and
that this is the client's responsibility. This is a documented limitation, not
a defect to be fixed later by adding a sanitizer.

A terminal is an interpreter too: `cairn(1)` scrubs archive text where it meets
one (D14).

### 7.4 Self-restriction

After archives are opened and the listener is bound, `cairnd` confines itself:
Landlock read-only on the archive directory and nothing else, and a `seccomp`
allowlist that drops everything the serving loop does not need. The steady-state
syscall set is small — `pread64`, `mmap`, accept/read/write on the listener, and
little more. As in clove, the allowlist is measured from a traced run rather
than guessed, and the tests exercise the serving workload under the live filter.
What was applied is reported by `/v1/status`.

The filter has two classes of denial: a syscall that is evidence of compromise
kills the process, and the open family fails with `EACCES` so that a library
asking the filesystem an advisory question does not take the daemon down. No
path can be opened either way. See D15.

### 7.5 OS sandbox

Two systemd units, system and user, installed by prefix. The system unit adds
`IPAddressDeny=any`, `RestrictAddressFamilies=AF_UNIX`, `ProtectSystem=strict`,
`ReadOnlyPaths` on the archive directory, and `PrivateNetwork=yes` where a unix
socket is used.

### 7.6 Not defended against

A compromised kernel; an attacker who already controls the daemon's account;
resource exhaustion beyond the specific bounds in §5 and §7.2; anything a
reverse proxy in front of cairn chooses to do; and the availability consequence
in §5.2. cairn makes no confidentiality claim about *which* entries an observer
sees requested.

## §8 Open questions

All five are resolved in [`DECISIONS.md`](DECISIONS.md). Kept here because the
rest of this document cites them by number, and because a decision is only
meaningful next to the question it answered.

1. **Decompressor crates.** Pure-Rust keeps the memory-safety story whole and
   the tree auditable, at a throughput cost; C bindings are faster but put a C
   decompressor on the hostile-archive path — the exact place §3.6 was written
   to avoid. → **D1: pure Rust** (`ruzstd`, `lzma-rs`).
2. **Namespaces.** Whether the API exposes namespaces at all, or presents a
   flat path space with the namespace resolved internally. → **D2: flat.**
3. **Auth failure indistinguishability.** See §7.2. → **D3: indistinguishable,
   checked before routing.**
4. **Cluster cache policy.** Size, eviction, per-archive or global. Interacts
   with the amplification ceiling. → **D4: one global LRU budget.**
5. **Suggestion semantics.** Title-prefix only, or prefix plus a normalized
   fold? Folding is a correctness rabbit hole and a dependency. → **D5:
   byte-exact prefix.**

## §9 Testing

Routine tests run without any archive present; a small crafted corpus is
committed, and large real archives are optional.

```
make test        unit, model, hostile-archive, and hostile-request tests
make smoke       daemon and CLI end to end over a crafted archive
make chaos       truncated files, archives replaced under a live daemon
make sandbox     the serving workload under the live seccomp filter
make lint        clippy, warnings denied
make fmt         rustfmt check
make man-lint    mdoc validation
make doc-lint    rustdoc links and warnings
make deps        dependency allowlist, licences, crate boundaries
make deny        cargo-deny: advisories, licences, bans, sources
make fuzz        cargo-fuzz, nightly toolchain
make fuzz-seed   fold the nightly corpus back into the committed seeds
```

Two fuzz targets, both first-class:

**A — archive.** Truncated headers; header fields pointing past EOF; MIME table
without terminator; MIME strings containing CR, LF, and NUL; cluster offsets in
descending order; self-referential and long redirect chains; zstd and LZMA2
bombs; entry counts inconsistent with pointer list lengths; malformed and
duplicate UUIDs.

**B — request.** Raw socket bytes: malformed request lines, absurd header
counts, oversized URIs, embedded NUL and CR, double-encoded and over-long
percent sequences, degenerate and overlapping `Range` values, and unicode
confusables in `{uuid}` and `q`.

Fuzzing is not only a pull-request gate. A nightly job runs both targets for
longer against a corpus that **persists and grows across runs**, minimises it,
and uploads it; `make fuzz-seed` folds what it found back into the committed
seeds. A corpus discarded at the end of every run only ever re-finds what the
seeds already reach. See [`../fuzz/README.md`](../fuzz/README.md).

CI additionally checks the dependency allowlist and fails if a crate crosses a
declared boundary without review.

## §10 Milestones

All five are complete; the project is pre-alpha and unaudited.

**M1 — parser.** `zimfmt` reads header, MIME table, pointer lists, dirents, and
uncompressed clusters, written from the spec. Fuzz target A exists and runs in
CI. No serving.

**M2 — decompression.** zstd and LZMA2 clusters, bounded output, decompressor
choice from §8.1 decided. Fuzz corpus extended.

**M3 — serving.** `cairnd` serves `/v1/status`, `/v1/archives`, and entry
content over a unix socket, with the §7.2 limits in place from the first commit
rather than retrofitted. Fuzz target B exists. `cairn(1)` speaks to it. Man
pages written.

**M4 — confinement.** Landlock and seccomp applied and reported; `sandbox
require` works; systemd units installed by prefix; `make sandbox` passes.

**M5 — the rest of the API.** Range requests, `HEAD`, suggestion, random,
archive metadata. `SECURITY.md`, `DEPENDENCIES.md`, and `docs/DECISIONS.md`
complete.

**Outstanding against M2:** verification against a real Wikipedia archive,
compared with `kiwix-serve` output for a sample of entries, and the decompressor
benchmark that D1 names as the thing that would reopen it. Neither is a code
gap; both need an archive too large to commit.

## §11 License

**ISC**, matching clove. Decided, not deferred.

The alternative was copyleft to enable reuse of Kiwix code. That reuse is
unavailable in the shape it was imagined and unnecessary given §3.6 and §4:

- Kiwix is **GPLv3-or-later**, not AGPL. AGPLv3 would not "match" it; AGPLv3
  and GPLv3 are combinable, so AGPL would have *permitted* the reuse, but the
  premise that AGPL mirrors Kiwix is wrong.
- With no libzim FFI, no FTS, no HTML rendering, no catalogue, and an
  independently written parser, there is nothing left worth copying. What cairn
  actually needs from the Kiwix ecosystem is the **format specification**,
  which is published openly and is not the licensed artifact.
- Copyleft would cover the entire tree for the sake of code that will never be
  imported, and AGPL §13 would additionally place a source-offer obligation on
  every appliance deployment — a cost paid by exactly the offline, air-gapped
  users cairn is for.

Therefore a standing rule, recorded in `DEPENDENCIES.md` and enforced in CI:
**no GPL- or AGPL-licensed code or crate enters this tree.** Not vendored, not
linked, not adapted, not consulted while writing `zimfmt`. Relicensing later
requires the consent of every contributor — trivial at zero contributors,
painful at twenty — so the rule is set now, while it is free.

*This reflects a reading of the licenses involved, not legal advice; confirm
with a lawyer before relying on it.*

## §12 Engineering standards and releases

### Code quality

- `#![forbid(unsafe_code)]` in every crate that does not need it. `unsafe`
  exists only in `sandbox` (syscalls) and `archive` (mmap), each site carrying
  a `SAFETY:` comment that CI lints for.
- `missing_docs` denied on library crates; rustdoc on every public item.
  Module docs explain *why*.
- clippy with warnings denied. The socket and IP types are in `clippy.toml`'s
  `disallowed-types`, so §7.1 is enforced at the type level and not only by
  crate name.
- `unwrap` and `expect` are lint-denied outside tests. Panics are bugs — except
  a third-party decoder's, which the parser contains (D11).
- Errors are small hand-written enums per module. No `anyhow`. Error text is
  written for the operator reading a log at 2 a.m.

### Documentation

Man pages are the primary user documentation: `cairnd(8)`, `cairn(1)`,
`cairn.conf(5)`, `cairn-api(7)`. Every page has a real EXAMPLES section. The
README stays short and defers to them. Documentation is brief and to the point:
a decision records what was decided, what it cost, and what would reopen it —
not the story of how it was found.

### Testing

- Aspiration: **test code volume exceeds source code volume.** Tracked, not
  gated.
- Hostile-input suites are first-class, not an afterthought, and every parser
  has a fuzz target.
- Chaos tests run in CI, not by hand.
- **Runnable by anyone:** `make test` from a clean checkout runs the whole
  suite with no infrastructure and no archive.

### Releases

- **Calendar versioning** ([calver.org](https://calver.org/)): a release is
  named for the month it was cut in — full year, zero-padded month, `YYYY.0M`,
  e.g. *2026.08*, with a counter appended for a second release in one month
  (*2026.08.1*). No major/minor/patch: the name says when, and promises nothing
  about compatibility. That promise belongs to the `/v1/` path and to the
  archive format.
- **Cutting one:** `api::VERSION` is the name; `Cargo.toml` carries the same
  release as `2026.8.0`, since semver forbids the leading zero. `crates/cairn`
  keeps its own copy because the CLI has no dependencies. Bump all three — a
  test fails otherwise — refresh the `cairn-api(7)` example, tag `v2026.08`.
- **Boring is good:** few, well-tested releases over frequent ones.
- **Culture of deletion:** every feature justifies its continued existence at
  each release. Removals are announced, not buried. The LOC count is allowed —
  encouraged — to go down.
