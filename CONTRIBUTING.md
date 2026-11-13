# Contributing to curst

Thanks for helping. This guide covers how the code is organised, the rules CI enforces, and the platform decisions behind the dependencies.

## Before you open a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov --workspace --summary-only
```

CI runs clippy and the tests on macOS, Linux and Windows, and the format check and coverage report on Linux. A pull request is merged only when all of them pass.

## Toolchain

- Rust is pinned in `rust-toolchain.toml` (`1.100.0`, with `clippy`, `rustfmt` and `llvm-tools-preview`). rustup installs it on the first `cargo` command, or explicitly with `rustup toolchain install`. Bumping it is a deliberate commit of its own.
- Coverage: `cargo install cargo-llvm-cov --locked`, then `cargo llvm-cov`. CI installs it with `taiki-e/install-action`.

## Architecture

The workspace has two crates:

- `curst-core`: the library. Request model, variables, storage, secrets, HTTP, curl and OpenAPI import. It never depends on a terminal, `ratatui`, `clap` or the clipboard.
- `curst`: the binary. TUI and headless CLI on top of the core.

Rules:

- **Dependencies point inward.** `curst` depends on `curst-core`, never the other way round.
- **The domain is pure.** `model` and `vars` do no I/O: no file paths, no network. Storage, secrets and HTTP sit behind traits (`CollectionStore`, `SecretStore`, `HttpExecutor`). The real implementations are plugged in at `main.rs`, and tests use in-memory fakes. The clipboard trait and `arboard` live in the binary.
- **One direction of data flow in the TUI.** Key event → `Action` → `update(state, action)` → `view(state)`. `update` is a plain function with no drawing and no I/O, so UI behaviour is unit-testable. Side effects (send a request, save a file) are returned as commands and run outside it.
- **Small public surface.** One responsibility per module. Items are private or `pub(crate)` unless another crate needs them.

## Rust rules

- No unsafe code: `unsafe_code = "forbid"` for the whole workspace.
- Errors are typed enums with `thiserror` in the core. `anyhow` is used only at the binary edge.
- No `unwrap`, `expect` or `panic!` in production code: the clippy lints `unwrap_used`, `expect_used` and `panic` are denied. Tests are exempt. `clippy.toml` allows `unwrap` and `expect` in tests, each crate root has `#![cfg_attr(test, allow(clippy::panic))]`, and integration test files under `tests/` start with `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`.
- Make invalid states unrepresentable: enums for `Method`, `Body` and `Auth`; newtypes for ids, variable names and secret keys. Secret values use a `Secret` type that zeroizes on drop and prints `***` in `Debug` and `Display`.
- `clippy::all` and `clippy::pedantic` are warnings, and CI denies warnings. Default `rustfmt` style.
- Every public item in `curst-core` has a doc comment (`#![warn(missing_docs)]`). Main entry points get doctests.
- Comments explain why the code does something non-obvious. They don't narrate history, plans or the obvious.
- Comments, doc comments included, start in lower case and never end with a full stop: `// retry once, the server drops idle connections`. Proper names and code identifiers keep their case.

## Tests

- Tests ship with the code, in the same pull request. Every bug fix comes with a test that fails before the fix.
- `curst-core` targets 85 % line coverage or more, measured with `cargo llvm-cov`. CI reports workspace coverage and fails when `curst-core` drops below the target (`--fail-under-lines 85`). `secrets/keychain.rs` is excluded because CI cannot run an OS keychain; it is kept as a thin adapter with no logic of its own.
- The TUI is tested through `update` unit tests and `insta` snapshots rendered with ratatui's `TestBackend`. After an intentional UI change, review new snapshots with `cargo insta review` (or run once with `INSTA_UPDATE=always` and read the diff). Never commit a `.snap.new` file.
- The repository forces LF line endings (`.gitattributes`) so snapshots are identical on every platform.
- Store tests never touch your real data: in-process tests pass a temporary folder with `Paths::under`, and anything that runs the binary sets `CURST_DATA_DIR` and `CURST_CONFIG_DIR` to a temporary folder. The two-process registry test re-runs its own test binary as the second process.
- HTTP tests run against `httpmock`, which answers plain HTTP and HTTPS on the same port. HTTPS tests trust `crates/curst-core/tests/fixtures/httpmock-ca.pem`, a copy of the CA in httpmock's `certs/ca.pem`; copy it again when httpmock is upgraded.

## Commits, branches and pull requests

Commit messages, branch names and pull request titles all follow [Conventional Commits](https://www.conventionalcommits.org/) and use the same types:

| Type | For |
|---|---|
| `feat` | a new feature |
| `fix` | a bug fix |
| `docs` | documentation only |
| `test` | adding or fixing tests |
| `refactor` | a code change that neither fixes a bug nor adds a feature |
| `perf` | a performance improvement |
| `build` | the build system or dependencies |
| `ci` | CI configuration |
| `chore` | anything else that doesn't touch `src` or tests |

The scope is optional and names the area touched, for example `core`, `tui`, `cli`, `secrets`, `http`, `openapi`. A breaking change adds `!` after the type or scope, and the commit body explains it in a `BREAKING CHANGE:` footer.

**Commits**: `<type>(<scope>): <summary>`, one logical change per commit. Write the summary in the imperative, in lower case, with no full stop: `fix(http): keep the body on a 307 redirect`.

**Branches**: `<type>/<short-description>`, in lower-case kebab case with no scope, branched from `main`: `feat/curl-paste`, `fix/redirect-body`, `docs/contributing`. If the branch closes an issue, put the issue number first: `fix/42-redirect-body`.

**Pull request titles**: the same format as a commit, `<type>(<scope>): <summary>`. Pull requests are squash-merged, and the title becomes the commit message on `main`, so the title is what ends up in the history and the changelog. Use the type of the most significant change. If a pull request needs two types, it is probably two pull requests.

### HTTP: `curl` 0.4.51 (`curl-sys` 0.4.91, libcurl 8.22)

- Features: `default-features = false`, with `static-curl`, `rustls` and `http2`.
- The same choice on every platform, in CI and in releases: a bundled, statically linked libcurl that uses rustls (`aws-lc-rs` crypto) and nghttp2. It needs no system libcurl or OpenSSL, so all platforms behave the same way. `cargo tree -i openssl-sys` finds nothing for macOS, `x86_64-unknown-linux-gnu` or `x86_64-pc-windows-msvc`. On Linux, `openssl-probe` is pulled in, but it only looks for certificate paths and links no OpenSSL.
- Certificates: rustls in libcurl has no certificate verifier by default. Every handle must set `SslOpt::native_ca(true)` so it uses the OS trust store. Without it, every HTTPS request fails with `failed to build client config: no server certificate verifier`. The probe got a 200 over HTTPS with this setting on macOS. Linux and Windows are *unconfirmed*.
- Against a local self-signed server: `ssl_verify_peer(false)` (`curl -k`) works, and so does `cainfo(<file>)`, but only when `native_ca` is off. With both set, libcurl ignores the CA file. A custom CA therefore replaces the OS trust store instead of adding to it.
- Building needs a C compiler (and, for `aws-lc-rs`, possibly CMake or NASM on some targets). The Windows MSVC and Linux builds are *unconfirmed*.

### OS keychain: `keyring-core` plus one store crate per platform

`keyring` 4.x splits into `keyring-core` and separate store crates. Its own docs tell applications that choose their stores to depend on these directly rather than on `keyring`. Our dependencies:

| Platform | Crate | Required feature |
|---|---|---|
| macOS | `apple-native-keyring-store` | `keychain` (without it the build fails with a `compile_error!`) |
| Windows | `windows-native-keyring-store` | none (*unconfirmed*) |
| Linux and other Unix | `zbus-secret-service-keyring-store` | `crypto-rust` (keeps OpenSSL out; *unconfirmed*) |

- The adapter (`secrets/keychain.rs`) creates the store for the current platform and registers it with `keyring-core` (*unconfirmed*: not exercised yet). If that fails (for example Linux without Secret Service), the error goes to the backend probe, which suggests the encrypted file instead.
- **Windows size limit**: `CRED_MAX_CREDENTIAL_BLOB_SIZE` is 2560 bytes. `set_password` stores UTF-16, which halves the limit to 1280 UTF-16 units, so the adapter stores UTF-8 bytes with `set_secret`, giving 2560 bytes. The store returns `Error::TooLong` beyond that. curst reports the limit and never truncates.

### YAML: `serde-saphyr` 2.x

- `serde_yaml` is deprecated, and `serde_yml` now describes itself as an unmaintained compatibility shim.
- `serde-saphyr` is pure Rust and written to never panic. It reads into `serde_json::Value`, which is all OpenAPI import needs. It resolves anchors and reads unquoted numeric keys such as `200:` under `responses` as the string `"200"`. Its default limits reject an alias bomb ("budget breached" at 250 001 nodes). That matters because specs can come from any URL.

### Paths: `etcetera` 0.11

- `choose_base_strategy()` already returns XDG paths on macOS (`~/.config`, `~/.local/share`); the Apple folders come only from `choose_native_strategy()`. On Windows it should return `%APPDATA%` (*unconfirmed*). curst uses `choose_base_strategy()`, and a unit test pins the result.

### File locking: `std::fs::File::lock`

- Stable since Rust 1.89, so no file-lock crate is needed. On macOS, a second handle's `try_lock` fails while the first holds the lock and succeeds once it is released. Linux and Windows are *unconfirmed* until a two-process locking test runs there.
