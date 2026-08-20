# cairn

A stack of stones raised so that whoever comes later can find the way without
asking anyone. Built once, read many times, needs no network to do its job.

cairn serves the contents of [ZIM](https://openzim.org) archives over a small
local HTTP API. It returns stored entries and metadata. It does not render a
website, rewrite HTML, inject a reader, or present a catalogue page.

The intended consumers are other programs: a local reader UI written by someone
else, a search tool, a script, an offline agent. A human with `curl` is a
supported consumer; a human with a browser and no client is not the target.

> **Pre-alpha and unaudited.** Nothing here has been reviewed by anyone but its
> authors. It works, it is tested, and it is not something to put in front of a
> hostile network yet.

## What it is not

Permanently, not "not yet":

- **No web UI, ever.** No HTML rendering, no CSS, no bundled JavaScript.
- **No HTML rewriting.** Links inside archived pages are returned exactly as
  stored. Clients that render entries must isolate the origin themselves; see
  [`SECURITY.md`](SECURITY.md).
- **No OPDS, ever.** `/v1/archives` is the entire discovery surface.
- **No writing ZIM files**, no scraping, no accounts, no TLS, no clustering,
  no plugins.

**No full-text search in 1.x.** This is a real capability gap against
`kiwix-serve`, and cairn offers title-prefix suggestion instead. Closing it
means a Xapian reader in Rust or a sidecar index, and both are their own
project.

## The API

```
GET  /v1/status                         daemon state, sandbox actually applied
GET  /v1/archives                       open archives: uuid, title, entry count
GET  /v1/archives/{uuid}                archive metadata from the M namespace
GET  /v1/archives/{uuid}/entry/{path}   entry content; Range supported
HEAD /v1/archives/{uuid}/entry/{path}   size and type without the body
GET  /v1/archives/{uuid}/random         one random entry path
GET  /v1/archives/{uuid}/suggest?q=     title-prefix suggestions
```

Every endpoint is specified in [`cairn-api(7)`](man/cairn-api.7). Archives are
addressed by the UUID in their header, so ids are stable across restarts,
renames and machines.

```console
$ cairn status | jq .sandbox.layers
$ cairn archives
$ cairn get 8b1f9c2e-... index.html > index.html
$ curl --unix-socket /run/cairn/cairn.sock http://cairn/v1/archives
```

## Two hostile inputs

The archive and the client are both untrusted, and neither is more privileged
than the other.

- **The parser is ours.** No FFI to `libzim`, no existing ZIM crate. It is
  written from the openZIM specification, small enough to review in one
  sitting, and every offset is checked against the file length.
- **The request parser is ours too.** HTTP/1.1 only, raw bytes in, every bound
  explicit, no request body accepted anywhere.
- **Both have a fuzz target**, and both are first-class.
- **The daemon confines itself** after archives are open and the listener is
  bound: Landlock read-only on the archive directory, and a seccomp allowlist
  measured from a traced run. `openat`, `clone`, `execve`, `socket` and
  `connect` are not on it.
- **Confinement is verifiable from outside.** `cairn status` reports what was
  actually applied, because a daemon that failed to confine itself otherwise
  looks identical to one that succeeded.

## Build and install

Linux 6.12 or later, on `x86_64`, `aarch64` or `riscv64`. Two of the three
confinement layers exist nowhere else, and portability is not accepted as a
reason to weaken them. The toolchain is pinned in `rust-toolchain.toml` and
builds are reproducible from the committed `Cargo.lock`.

```console
$ make build
$ sudo make install PREFIX=/usr/local
$ sudo systemctl enable --now cairnd
```

`make install` places `cairnd`, `cairn`, the four man pages, an example
`cairn.conf`, and both systemd units under `PREFIX`.

## Configuration

```
listen = unix:/run/cairn/cairn.sock
archive_dir = /srv/zim
sandbox = require
```

Every key, with its default, is in [`cairn.conf(5)`](man/cairn.conf.5); a
fully commented example is in [`contrib/cairn.conf`](contrib/cairn.conf).
Start with `sandbox = best-effort`, read `cairn status`, then set
`sandbox = require` once you know your kernel provides what you asked for.

## Development

```console
$ make test        # unit, model, hostile-archive and hostile-request tests
$ make smoke       # daemon and CLI end to end over a crafted archive
$ make chaos       # truncated files, archives replaced under a live daemon
$ make sandbox     # the serving workload under the live seccomp filter
$ make lint        # clippy, warnings denied
$ make fmt         # rustfmt check
$ make man-lint    # mdoc validation
$ make doc-lint    # rustdoc links and warnings
$ make deps        # dependency allowlist, licences, crate boundaries
$ make fuzz        # cargo-fuzz, nightly toolchain
```

Routine tests need no archive present: `crates/testutil` crafts them. Seven
third-party crates, each with a reason in [`DEPENDENCIES.md`](DEPENDENCIES.md).
Open questions from the scope are resolved in
[`docs/DECISIONS.md`](docs/DECISIONS.md).

### Layout

| Crate | Responsibility |
|---|---|
| `zimfmt` | Pure parser. Header, MIME table, pointer lists, dirents, clusters, decompression. No I/O. Fuzz target A. |
| `archive` | Opening and holding archives: mmap lifetime, UUID identity, redirect resolution, lookup, cluster cache. |
| `api` | HTTP surface. No dependencies at all. Fuzz target B. |
| `sandbox` | Landlock and seccomp construction, and reporting of what was applied. |
| `cairnd` | The daemon: config, init ordering, confinement, serving. |
| `cairn` | Control CLI, speaking the same API as any other client. |

`zimfmt` has no dependency that can perform I/O; `api` has none at all. These
boundaries are enforced by `ci/check-boundaries.sh`, not by convention.

## Licence

[ISC](LICENSE). No GPL- or AGPL-licensed code or crate enters this tree — not
vendored, not linked, not adapted, not consulted while writing `zimfmt`. What
cairn needs from the Kiwix ecosystem is the format specification, which is
published openly and is not the licensed artifact.
