# Project specs, tools, build & conventions

> **Purpose.** This file holds every project-specific detail an AI coding
> agent (or human contributor) needs to work in this repository. It is
> **loaded on demand by the metaprompt in [`AGENTS.md`](../../AGENTS.md)** —
> together with [`research.md`](research.md) (methodology) and
> [`devmind.md`](devmind.md) (engineering mindset).
>
> **Evidence rules.** Every factual claim here is backed by evidence: a
> file in this repo (cite as `file:line`), git history, CI configuration,
> or a command that was actually run. Documented commands carry a
> verification marker — `<!-- verified YYYY-MM-DD: <observation> -->` —
> or are marked `<!-- UNVERIFIED — source: <where it was read> -->`.
> Sections without evidence are deleted, never left as empty stubs.

## Build Commands

```bash
# Build everything (debug profile, all 5 crates of the workspace)
cargo build --workspace

# Build everything in release mode (the README-documented variant)
cargo build --release --workspace

# Run the CLI binary (workspace default-run points at xodus-cli)
cargo run -- --help

# Run the service binary
cargo run --bin xodus-service

# Formatting gate (same command CI uses)
cargo fmt --check --all

# Lint gate (same pipeline CI uses; the annotator exits non-zero on error-level
# diagnostics, so run it under `set -o pipefail` to get a CI-equivalent status)
cargo clippy --workspace --message-format=json | python3 scripts/cargo-annotate.py x86_64-unknown-linux-gnu

# Full test suite
cargo test

# A single test
cargo test -p msixvc-common test_version_cmp

# Build the tokio-console profiling variant.
# Always pass -p: BOTH xodus-cli and xodus-service define the `tokio_console`
# feature, so --features without -p is ambiguous from the workspace root.
cargo build -p xodus-cli --features tokio_console
```

<!-- verified 2026-10-12: cargo clippy --workspace --message-format=json | python3 scripts/cargo-annotate.py x86_64-unknown-linux-gnu -> exit 0, 3 warnings, 1m04s cold -->
<!-- verified 2026-10-12: cargo test -> exit 0 (42 passed, 1 ignored) -->
<!-- verified 2026-10-12: cargo test -p msixvc-common test_version_cmp -> ok, 11 filtered out -->
<!-- verified 2026-10-12: cargo run -- --help -> prints usage with 10 subcommands -->
<!-- verified 2026-10-12: cargo fmt --check --all -> exit 0 -->
<!-- verified 2026-10-12: cargo build --workspace -> exit 0 (executed via the dev-profile compilation of cargo test / cargo clippy) -->
<!-- UNVERIFIED — source: README.md:71,89. `cargo build --release --workspace` and the README's
     `RUSTFLAGS="--cfg tokio_unstable" cargo run --features tokio_console` were not executed in this
     session; the README form of the tokio_console invocation cannot select a crate from the workspace
     root because the feature exists in xodus-cli AND xodus-service. -->
<!-- UNVERIFIED — source: README.md:83. `cargo run --bin xodus-service` not executed (it binds a Unix
     socket and blocks until interrupted). -->

## Specifications & references

Normative/reference sources this project's behaviour is checked against:

| Reference | Role | Pointer |
|---|---|---|
| In-repo Xbox protocol docs | Reverse-engineered protocol documentation (MSA/XSTS/Xboxlive/titlestorage/gameruntime/gamepass) — wiki-mirrored | `docs/xbox/` ([`MSAUserLogin.md`](../xbox/MSAUserLogin.md), `xboxlive.md`, `xboxservices.md`, `gameruntime.md`, `gamepass.md`, `titlestorage.md`, `xbox.md`) |
| In-repo Xodus docs | Login/licensing/CLEP/device flow + architecture — wiki-mirrored | `docs/xodus/` ([`architecture.md`](../xodus/architecture.md), `clep.md`, `device.md`, `licenses.md`, `login.md`, `xodus.md`) |
| LukeFZ/XvdTool.Streaming | Reference implementation credited upstream | README.md:122 |
| LukeFZ/CikExtractor | Reference implementation; provenance attribution comment in derived code | README.md:122; `crates/xodus/src/licensing/splicense.rs:1-4` |
| xal-rs | XBOX Live auth library (device/user/XSTS auth) — git dep pinned to a rev | `crates/xodus/Cargo.toml:8` (xodus-gaming/xal-rs, rev `6aa2420f8f0d5e6f7460ff13f399037583a942f4`) |
| eappx-rs | EAppx/EMSIX package parsing — git dep, not rev-pinned | `crates/xodus-cli/Cargo.toml:10` (tuxuser/eappx-rs) |
| ntfs fork | NTFS parsing, workspace-wide `[patch.crates-io]` to the xodus-gaming fork | `Cargo.toml:19-20` |
| xgameruntime (companion repo) | Open-source `xgameruntime.dll` implementation the service talks to over IPC | `docs/xodus/architecture.md:9` |
| Wine fork (companion repo) | Xodus Wine fork with `xgameruntime.dll` patched in | `docs/xodus/architecture.md:10` |
| wry platform notes | Login webview has platform-specific requirements | README.md:63 |

The `docs/` directory is synced to the GitHub wiki automatically (see CI).

## Linting / Formatting

- **rustfmt**, default configuration (no `rustfmt.toml` / `.rustfmt.toml`
  anywhere in the repo — verified absence 2026-10-12). Gate:
  `cargo fmt --check --all`, CI job `fmt` (`.github/workflows/rust.yml:14`).
- **clippy**, default set (no `clippy.toml` in the repo — verified absence
  2026-10-12). CI runs
  `cargo clippy --workspace --message-format=json | python3 scripts/cargo-annotate.py <target>`
  (`.github/workflows/rust.yml:43-47`). The annotator converts warnings to
  GitHub PR annotations with target-labels and **exits non-zero when any
  error-level diagnostics appear** (`scripts/cargo-annotate.py:1-14`).
- If skipped: the PR CI job goes red — fmt via the check job, clippy via
  annotations plus the pipeline's exit status (`set -o pipefail`,
  `rust.yml:46`).
- Current baseline (2026-10-12): clippy exit 0 with 3 warnings —
  `clippy::collapsible_if` at `crates/xodus-cli/src/commands/extract_eappx.rs:63-68`,
  `clippy::too_many_arguments` at `crates/xodus-cli/src/commands/streaming.rs:154-168`
  and `:228-241`.

## Coding Conventions

Observed, not prescribed (sampled across all 5 crates):

- All crates on **Rust edition 2024** and `rust-version = "1.98.0"`
  workspace-wide (`Cargo.toml:5-6`, e.g. `crates/msixvc/Cargo.toml:4`).
- Workspace members `crates/*`, resolver 3 (`Cargo.toml:1-3`). Common deps
  declared once in `[workspace.dependencies]` (`Cargo.toml:8-17`) and
  referenced as `tokio = { workspace = true }` etc. in each crate manifest.
- Error types via **thiserror** derive (e.g.
  `crates/msixvc/src/models/xsp/structs.rs`,
  `crates/xodus/src/licensing/content.rs`).
- CLI parsing via **clap 4 derive** (`#[derive(Parser, Subcommand)]`,
  `crates/xodus-cli/src/main.rs:8-22`).
- Logging via **tracing** + `tracing_subscriber` registry;
  `EnvFilter::from_env("XODUS_LOG")` in xodus-service
  (`crates/xodus-service/src/main.rs:18-20`).
- Flat `pub mod` lists as the only content of lib.rs
  (`crates/msixvc/src/lib.rs:1-9`, `crates/msixvc-common/src/lib.rs:1-2`),
  except xodus, which also holds constants, re-exports and the proto
  include (`pub use xal;`, `include!(concat!(env!("OUT_DIR"), "/xodus.rs"))`,
  `crates/xodus/src/lib.rs:10-18`).
- Magic-byte constants as hex consts, little-endian ASCII:
  `const XML_MAGIC: u32 = 0x58445358; const PROTO_MAGIC: u32 = 0x58445350;`
  (`crates/xodus-service/src/main.rs:15-16`).
- Unit tests are inline `#[cfg(test)]` modules in the same file as the code
  (18 files, e.g. `crates/msixvc-common/src/parse.rs`, `crates/msixvc/src/streaming.rs`).
- No blanket licence headers in source files; only provenance attributions
  where code derives from another project
  (`crates/xodus/src/licensing/splicense.rs:1-4`).
- No license-header, include-order or brace-specific config — rustfmt
  defaults apply (see Linting).

## Commit Message Style

Derived from `git log --format='%s' -n 50` (2026-10-12). Accepted form:
`<type> [: <scope>] <summary> (#<PR>)`. **Nothing is enforced** — one
unprefixed commit exists in the recent 50.

Real examples verbatim:

```
assets: add svg icon variants (#213)
msixvc: add `HashTreeStream`  (#182)
fix: handle malformed and oversized SOAP responses (#185)
fix(cli): download --dry-run only printed the first selected file's URL (#133)
feat(breaking): run command (#72)
```

Observed types: `feat` (incl. `feat(breaking)`), `fix` (scope optional:
`fix(cli)`, `fix(licensing)`), `docs`, `ci`, `chore`, `tech`, `misc`,
`refactor`, and crate/area names as prefix (`crypt:`, `msixvc:`). The
`(#NN)` suffix is added by GitHub squash-merges.

## Common Workflows

1. **Setup**: Rust ≥ 1.98 (`rustup update`), `protoc` in PATH (workspace
   build needs it — see `build.rs` in Generated artefacts), and on Linux
   `libwebkit2gtk-4.1-dev` for the CLI login webview (CI does
   `sudo apt install libwebkit2gtk-4.1-dev protobuf-compiler`,
   `.github/workflows/rust.yml:28`).
2. **Build once**: `cargo build --workspace`.
3. **Run one test**: `cargo test -p msixvc-common test_version_cmp`.
4. **Reproduce CI locally** (Linux/x86_64):
   ```bash
   set -o pipefail
   cargo fmt --check --all
   cargo clippy --workspace --message-format=json | python3 scripts/cargo-annotate.py x86_64-unknown-linux-gnu
   cargo test
   ```
5. **Run the CLI**: `cargo run -- --help` then e.g. `cargo run -- login`.
6. **Run the service**: `cargo run --bin xodus-service` (binds
   `${runtime_dir}/xodus.sock`, blocks until interrupted).
7. **Profile with tokio-console**: `cargo build -p xodus-cli --features tokio_console`,
   then run with `RUSTFLAGS="--cfg tokio_unstable"` in the environment used
   at compile time and view via tokio-console. Runtime behaviour UNVERIFIED
   (build-path only) — README.md:88-90.

## Architecture Overview

Data flow (derived from the code, README.md:47-58 and
`docs/xodus/architecture.md:4-10`):

1. **xodus-cli** (bin, clap subcommands) is the user-facing driver: login
   (webview via `wry`/`tao`), download/streaming/extract, license dumping,
   CLEP generate/decrypt, sp-license decoding.
2. **xodus** (rlib) holds the Xbox API stack: device/user login and XSTS
   auth delegated to `xal` (xal-rs), licensing, CLEP challenge generation,
   `TokenManager`, and the prost-generated proto types
   (`crates/xodus/src/api.rs`, `licensing.rs`, `clep.rs`, `tokens.rs`).
3. **msixvc** (rlib) parses, downloads and decrypts MSIXVC/XSP containers:
   GPT partition handling, the (patched-fork) NTFS volume, xvd support,
   hash-tree streaming and AES crypto (`crates/msixvc/src/xsp.rs`,
   `streaming.rs`, `hash_tree.rs`, `crypt.rs`, `xvd.rs`).
4. **msixvc-common** (rlib) provides a fixed-length binary structure
   parser and Package Resource Index (PRI) parsing
   (`crates/msixvc-common/src/parse.rs`, `resources.rs`,
   example `examples/pri_parse.rs`).
5. **xodus-service** (bin) binds `${runtime_dir}/xodus.sock` via
   `tokio::net::UnixListener` (`crates/xodus-service/src/main.rs:52-61`)
   and dispatches XML/PROTO-magic packets (`0x58445358` / `0x58445350`,
   `crates/xodus-service/src/main.rs:15-16`) — the IPC integration point
   for `xgameruntime.dll` (see architecture doc).

Key files:

| File | Purpose |
|---|---|
| `crates/xodus/src/lib.rs` | Core module list, endpoint constants, proto include |
| `crates/xodus/src/api.rs` | Xbox API call abstractions |
| `crates/xodus/src/tokens.rs` | `TokenManager` used by CLI and service |
| `crates/msixvc/src/streaming.rs` | Streaming download algorithm |
| `crates/msixvc/src/crypt.rs` | MSIXVC decryption |
| `crates/msixvc-common/src/parse.rs` | Fixed-length binary structure parsing |
| `crates/xodus-service/src/main.rs` | Unix-socket IPC loop, magic constants |
| `crates/xodus/build.rs` | prost proto compilation (see Generated artefacts) |

## Testing

- Unit tests live **inline** in `#[cfg(test)]` modules; there are no
  `tests/` integration directories and no golden-file fixtures in the
  repo. Test data sits in the test functions themselves.
- Collected baseline (2026-10-12, `cargo test` exit 0): test binaries with
  18, 12 and 13 tests (12 passed, 1 ignored) plus empty binaries for the
  other crates.
- The one ignored test is live-credential network auth:
  `#[ignore] #[tokio::test] async fn test_minecraft_win_auth`
  (`crates/xodus/src/auth.rs:189-193`) — it calls
  `crate::secrets::init_secrets()` and requires real credentials.
- Run everything: `cargo test`. Run one case:
  `cargo test -p <crate> <test_name>` (verified: `cargo test -p msixvc-common test_version_cmp`).
- Expected output is asserted inline; there is no
  regenerate-golden-file step.

## CI

`.github/workflows/`:

| Workflow | Trigger | Does |
|---|---|---|
| `rust.yml` ("Rust check") | push to `main`, all PRs | job `fmt`: `cargo fmt --check --all`; job `check`: matrix `x86_64-unknown-linux-gnu` (ubuntu-latest) / `aarch64-apple-darwin` (macos-latest), installs protobuf + linux webkit deps, `rustup update`, cargo cache keyed on `Cargo.lock` hash (`~/.cargo/*`, `target/`), clippy with annotator, `cargo test` |
| `wiki.yml` ("Wiki sync") | push to `main`, paths `docs/**` | rsyncs `docs/` into this repository's GitHub wiki, regenerates `_Sidebar.md` from the directory tree, bot-commits (`.github/workflows/wiki.yml:27-47`) |

Reproduce locally: see Common Workflows step 4.

## Runtime Configuration / Usage

- **xodus-cli subcommands** (from `cargo run -- --help`, 2026-10-12):
  `download`, `license`, `extract`, `extract-eappx`, `login`, `logout`,
  `streaming`, `run`, `clep` (generate/decrypt variants),
  `sp-license`, `help`.
  ⚠ **README drift**: the README.md:102-118 usage block lists only
  `download license extract login streaming clep sp-license help` — it
  predates `extract-eappx` (#138), `logout` (#110) and `run` (#72).
- **Environment / flags**:
  - `XODUS_LOG` — env filter for the tracing subscriber
    (`crates/xodus-service/src/main.rs:19`).
  - `RUSTFLAGS="--cfg tokio_unstable"` required for `console-subscriber`
    profiling variants (README.md:88-90).
- **Service socket**: `${runtime_dir}/xodus.sock`, bound by
  `UnixListener::bind` (`crates/xodus-service/src/main.rs:52,61`).
- **Secrets**: OS keyring backends — Linux `dbus-secret-service-keyring-store`
  (`crates/xodus/Cargo.toml:42`), macOS `apple-native-keyring-store`
  (`crates/xodus/Cargo.toml:45`), plus the dev-only
  `key-chain-file` cargo feature (`crates/xodus/Cargo.toml:55-56`,
  introduced by commit "feat: key-chain-file for development (#111)").

## Dependencies

- **Workspace manifests**: root `Cargo.toml:8-17` — base64, chrono, reqwest
  (json, stream), thiserror, tokio (full, tracing), uuid, tracing,
  tracing-subscriber (env-filter), console-subscriber.
- **Git dependencies** (all reverse-engineering-related forks/tools):
  `xal` from xodus-gaming/xal-rs pinned to
  rev `6aa2420f8f0d5e6f7460ff13f399037583a942f4` (`crates/xodus/Cargo.toml:8`);
  `eappx` from tuxuser/eappx-rs, **not rev-pinned** (`crates/xodus-cli/Cargo.toml:10`);
  `[patch.crates-io]` replaces the `ntfs` crate with
  xodus-gaming/ntfs.git unpinned (`Cargo.toml:19-20`).
- **System requirements**: `protoc` (needed by the workspace build —
  `prost_build` in `crates/xodus/build.rs`), `libwebkit2gtk-4.1` on Linux
  for the login webview (CI: `rust.yml:28`).
  ⚠ **README drift**: README.md:64 states only "xodus-service relies on
  `protoc`" — in reality the compilation happens in the `xodus` crate's
  build.rs, so every workspace build (including xodus-service, which
  depends on `xodus`) needs protoc.
- **CPU-feature pinning**: `.cargo/config.toml:4-8` enables
  `+aes,+ssse3` on x86_64 and `+aes` on aarch64. Consequence
  (README.md:92-98): the binaries crash with an illegal-instruction error
  on CPUs without those instructions — do not remove without intent
  (added for AES performance, see `.cargo/config.toml:1-2` and PR #60).
- **Notable CLI deps**: `tao` 0.37 + `wry` 0.56 (webview login),
  `inquire` (interactive prompts), `indicatif` (progress bars)
  (`crates/xodus-cli/Cargo.toml`).

## Source Layout

```
run from repo root; build output goes to ./target/{debug,release}

├── assets/                     icons + docs diagram sources
├── crates/
│   ├── msixvc/                 [rlib] MSIXVC/XSP parsing, download, decryption
│   ├── msixvc-common/          [rlib] binary-structure parser, PRI resources (+ examples/)
│   ├── xodus/                  [rlib] Xbox API stack (auth via xal, licensing, CLEP, tokens), proto/
│   ├── xodus-cli/              [bin]  CLI driver
│   └── xodus-service/          [bin]  Unix-socket service for xgameruntime.dll IPC
├── docs/                       protocol + architecture docs (wiki-mirrored)
│   ├── xbox/                   MSA/login/XSTS/titlestorage/gameruntime docs
│   └── xodus/                  CLEP, licences, device, login, architecture
├── scripts/
│   ├── cargo-annotate.py       CI: clippy JSON -> PR annotations
│   └── send_msa_token_request.py  manual XML/PROTO packet tool for testing
└── .github/workflows/          rust.yml (check/test), wiki.yml (wiki sync)
```

⚠ **README drift**: README.md:49-55 draws the same tree under top-level
paths (`msixvc`, `xodus`, …) and omits `msixvc-common`; the crates actually
live under `crates/` since "misc: move crates to subdirectory (#98)".

## Generated artefacts (never hand-edit)

| Generated file | Source | Regeneration |
|---|---|---|
| `$OUT_DIR/xodus.rs` (prost-generated proto code, included by `crates/xodus/src/lib.rs:14-18`) | `crates/xodus/proto/xodus/common.proto` | automatic via `cargo build` → `crates/xodus/build.rs:1-4` (`prost_build::compile_protos(&["./proto/xodus/common.proto"], ["./proto"])`) — verified: every in-session build/test/clippy run compiled it |

No repo-tracked generated files. (The wiki `_Sidebar.md` is generated
inside the CI job, outside this repository.)

## Verification log

| Command | Status | Evidence | Date |
|---------|--------|----------|------|
| `rustc --version` / `cargo --version` | verified | 1.99.0 / 1.99.0 (workspace needs ≥ 1.98.0) | 2026-10-12 |
| `protoc --version` | verified | libprotoc 36.1 (`/usr/bin/protoc`) | 2026-10-12 |
| `cargo fmt --check --all` | verified | exit 0, no output | 2026-10-12 |
| `cargo clippy --workspace --message-format=json \| python3 scripts/cargo-annotate.py x86_64-unknown-linux-gnu` | verified | exit 0, 3 warnings, 1m04s | 2026-10-12 |
| `cargo test` | verified | exit 0; 42 passed, 1 ignored (auth.rs:189) | 2026-10-12 |
| `cargo test -p msixvc-common test_version_cmp` | verified | ok; 11 filtered out | 2026-10-12 |
| `cargo run -- --help` | verified | usage with 10 subcommands printed | 2026-10-12 |
| `cargo build --workspace` | verified | exit 0 (via dev-profile compilation in the test/clippy runs) | 2026-10-12 |
| `find . -name 'rustfmt.toml' -o -name '.rustfmt.toml'` | verified | no match anywhere | 2026-10-12 |
| absence of `clippy.toml` / licence headers / tests/ dirs | verified | greps returned only provenance comment (splicense.rs:1-4), inline cfg(test) only, one #[ignore] | 2026-10-12 |
| `cargo build --release --workspace` | UNVERIFIED | read in README.md:71, not executed (long release build) | 2026-10-12 |
| `RUSTFLAGS="--cfg tokio_unstable" cargo run --features tokio_console` | UNVERIFIED | read in README.md:89; also ambiguous without `-p` (feature defined in xodus-cli AND xodus-service) | 2026-10-12 |
| `cargo run --bin xodus-service` | UNVERIFIED | read in README.md:83; blocks on binding the socket | 2026-10-12 |