# Dependencies

Seven third-party crates. Every one has a reason, a licence, and a note on what
it is allowed to touch. `ci/check-deps.sh` fails when a crate appears that is
not in the table below; `ci/check-boundaries.sh` fails when a crate reaches
past the dependencies its own crate is allowed.

## Standing rule

**No GPL- or AGPL-licensed code or crate enters this tree.** Not vendored, not
linked, not adapted, not consulted while writing `zimfmt`. cairn is ISC, and
relicensing later would need every contributor's consent. CI enforces this on
the declared licence of every crate in `Cargo.lock`.

## Runtime

| Crate | Version | Licence | Used by | Why, and what it can reach |
|---|---|---|---|---|
| `ruzstd` | 0.9 | MIT | `zimfmt` | Zstandard decoding for cluster bodies. Pure Rust, so the hostile-archive path stays memory-safe. Operates on slices; no I/O, no allocation beyond the output we bound. |
| `lzma-rs` | 0.3 | MIT | `zimfmt` | LZMA2/xz decoding for older archives. Pure Rust, same reason. `#![forbid(unsafe_code)]` upstream. Known to panic on a crafted footer (`docs/DECISIONS.md` D11); `zimfmt` catches it. |
| `byteorder` | 1.5 | Unlicense OR MIT | (via `lzma-rs`) | Integer reads inside the decoder. No I/O of its own. |
| `crc` | 3.4 | MIT OR Apache-2.0 | (via `lzma-rs`) | xz stream checksums. Pure computation. |
| `crc-catalog` | 2.5 | MIT OR Apache-2.0 | (via `crc`) | Table of CRC parameters. Data only. |
| `memmap2` | 0.9 | MIT OR Apache-2.0 | `archive` | Maps archives read-only. The one place `unsafe` touches the filesystem, justified at the call site. Cannot open a socket or spawn anything. |
| `libc` | 0.2 | MIT OR Apache-2.0 | `sandbox`, `cairnd` | Landlock, seccomp, `getrandom`. Syscall declarations; the calls cairn makes are the ones written here. |

`api` has **no dependencies at all**, deliberately: it parses hostile request
bytes, and its whole surface should be readable in one sitting. `cairn`, the
CLI, has none either.

## Development only

| Crate | Version | Licence | Why |
|---|---|---|---|
| `testutil` (this repo) | — | ISC | Crafts ZIM archives for tests. Never a dependency of `cairnd` or `cairn`. |
| `libfuzzer-sys` | 0.4 | MIT OR Apache-2.0 OR NCSA | Fuzz harness for targets A and B. Built only under `cargo fuzz`, in a workspace of its own (`fuzz/`). |
| `arbitrary` | 1.4 | MIT OR Apache-2.0 | Pulled in by `libfuzzer-sys`. Same scope. |

## What is deliberately absent

- **No `libzim` FFI, and no existing ZIM crate.** The parser touches
  attacker-controlled bytes; it is written here, from the specification.
- **No `serde`, no JSON crate.** The API emits a handful of documented shapes;
  `api::Json` is fifty lines and cannot be surprised by a `Deserialize` impl.
- **No HTTP crate, no async runtime.** HTTP/1.1 only, thread per connection,
  a fixed pool. Nothing in the tree can open an outbound socket.
- **No C decompressors (`zstd`, `xz2`).** Faster and more exercised, but they
  put C on the hostile-archive path, which is the one thing the parser exists
  to avoid. Revisit only with a benchmark that says it matters, recorded in
  `docs/DECISIONS.md`.
- **No TLS, no compression of responses, no logging framework.**

## Adding one

1. Say what it does that the standard library will not.
2. Say which crate uses it and which boundary it sits behind.
3. Check the licence, and the licences of everything it pulls in.
4. Add a row above, then run `make deps`.

Anything that can open a socket, spawn a process, or touch the filesystem
outside the archive directory needs more than a row: it needs an argument in
`docs/DECISIONS.md` for why the boundary still holds.
