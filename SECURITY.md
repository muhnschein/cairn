# Security

cairn is **pre-alpha and unaudited**. It has not been reviewed by anyone but
its authors. Treat the confinement described here as a design, not a promise.

## Reporting

Open a GitHub security advisory on the repository, or email the maintainer
listed there. Please include the archive or request bytes that reproduce the
problem — a crafted `.zim` or a raw request is worth more than a description.
There is no bounty.

## Threat model

**Two hostile inputs, not one.** A ZIM downloaded from a mirror is untrusted
the way a torrent is; a request arriving on the socket is untrusted the way a
peer message is. Neither is more privileged than the other, and both have a
fuzz target (`fuzz/fuzz_targets/archive.rs`, `fuzz/fuzz_targets/request.rs`).

### Defended against

- **A crafted archive.** Every offset read from the file is checked against the
  file length before use; every index against its declared count. Redirect
  chains are followed to a fixed depth and then abandoned. Decompressed cluster
  size is bounded by configuration. A malformed region produces `503` for that
  archive, never a `500` and never a panic.
- **A panicking decoder.** The decompressors are third-party code on the
  hostile-archive path, and one of them panics on a crafted xz footer. The
  parser catches the unwind and reports a decoding failure, and a worker
  catches anything that escapes the serving path so a panic costs a connection
  rather than a thread. See `docs/DECISIONS.md` D11.
- **A crafted request.** Raw bytes in, with no assumption that a well-formed
  request is the common case. Request line, header block, header count,
  connection count, timeouts, keep-alive length and per-connection request rate
  are all bounded. No request body is accepted at all.
- **Header injection through archive data.** MIME types come from the archive's
  own table and end up in a response header. Every value written into a header
  is validated against a strict token grammar; anything containing CR, LF, NUL
  or a non-token byte becomes `application/octet-stream`. Paths are
  percent-encoded before they reach `X-Cairn-Path`. This is the sharpest edge
  where the two hostile inputs meet, and it has its own test module.
- **Path handling.** ZIM paths are keys into a table, not filesystem paths, so
  traversal cannot escape. Decoding is still canonical: percent-decode exactly
  once, reject `%00`, reject control bytes, reject non-canonical UTF-8, and
  never re-decode a decoded result.
- **Amplification.** A tiny entry inside a large compressed cluster is a cheap
  request and an expensive decode. A bounded global cluster cache plus a
  per-connection request rate ceiling, both configurable, both with documented
  defaults.
- **Escalation after a bug.** Landlock leaves the archive directory readable
  and nothing else writable anywhere; seccomp denies `execve`, `socket`,
  `connect`, `bind` and `clone` by killing the process, and the open family
  (`open`, `openat`, `openat2`) by failing it with `EACCES`. No path can be
  opened either way — the difference is that a library probing `/proc` for a
  tunable gets an error instead of taking the daemon down with it. A
  memory-safety bug in the parser buys an attacker a process that can read
  archives it could already read.

### Not defended against

- A compromised kernel.
- An attacker who already controls the daemon's account.
- Resource exhaustion beyond the specific bounds in `cairn.conf(5)`.
- Anything a reverse proxy in front of cairn chooses to do.
- Availability when an archive is truncated or replaced under a running daemon:
  the mapping faults, and the process dies rather than answering wrongly. See
  `docs/DECISIONS.md` D8.
- Traffic analysis. cairn makes no confidentiality claim about *which* entries
  an observer sees requested.

## Rendering entries is the client's problem

cairn returns archived HTML unmodified. It does not rewrite links, strip
scripts, or sanitize anything, and it will not: that would put an HTML parser
in the hot path. A browser-based client that renders an entry is rendering
attacker-controlled markup and **must do so in an isolated origin**.

`X-Content-Type-Options: nosniff`, `Cross-Origin-Resource-Policy: same-origin`
and a `Content-Security-Policy` are sent with entry content. They are what a
server can do without parsing content; they are not origin isolation.

## Verifying confinement

A daemon that failed to confine itself looks identical to one that succeeded
unless it says so. It says so:

```
$ cairn status | jq .sandbox
{
  "required": true,
  "layers": [
    {"name": "no_new_privs", "state": "applied", "detail": null},
    {"name": "landlock", "state": "applied", "detail": "abi 5"},
    {"name": "seccomp", "state": "applied", "detail": "49 syscalls, 3 denied, kill on violation, 111 instructions"}
  ]
}
```

Set `sandbox = require` once you know your kernel provides what you asked for;
it turns a partially applied sandbox into a refusal to start.

## Guarantees this project makes about itself

- No component can open an outbound socket. `ci/check-boundaries.sh` fails if a
  crate crosses a boundary from §5 of [the scope](docs/SCOPE.md), and
  `clippy.toml` refuses the calls themselves.
- No name is resolved. A hostname in `listen` is refused at the parse; the
  resolver is never reached.
- No component writes to the archive directory. There is no upload endpoint, no
  archive management, and no mutation anywhere in the API surface.
- No process is spawned.
- Every crate in the tree is listed in `DEPENDENCIES.md` with a reason.
  `ci/check-deps.sh` fails when one appears that is not.
- `unsafe` appears only at the mmap boundary and in the sandbox syscalls, with
  a justification at each site.
