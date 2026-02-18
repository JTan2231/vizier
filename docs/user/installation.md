# Installing Vizier (from a clone)

Vizier can be installed directly from this repository. The installer builds the
`vizier` binary and installs bundled agent shim scripts plus sectioned man pages
into a standard prefix layout.

## Prerequisites

- Rust toolchain (`cargo`)
- `git` (for repository setup/inspection and normal developer workflows; Vizier runtime Git operations run through in-process `libgit2` helpers)
- `jq` (recommended): required by the bundled `filter.sh` scripts used to render agent JSONL progress

## Quick start (user prefix)

```sh
PREFIX="$HOME/.local" ./install.sh
```

This installs:

- `"$HOME/.local/bin/vizier"`
- `"$HOME/.local/share/vizier/agents/*"`
- `"$HOME/.config/vizier/workflows/{draft,approve,merge}.toml"` (or platform equivalent)
- `"$HOME/.local/share/man/man1/vizier.1"`
- `"$HOME/.local/share/man/man1/vizier-jobs.1"`
- `"$HOME/.local/share/man/man5/vizier-config.5"`
- `"$HOME/.local/share/man/man7/vizier-workflow.7"`
- `"$HOME/.local/share/man/man7/vizier-workflow-template.7"`

## Initialize a repository

Inside a Git repository, run:

```sh
vizier init
```

`vizier init` is idempotent. It ensures durable initialization markers exist at:

- `.vizier/narrative/snapshot.md`
- `.vizier/narrative/glossary.md`
- `.vizier/config.toml`
- `.vizier/workflows/{draft,approve,merge,commit}.toml`
- `.vizier/prompts/{DRAFT,APPROVE,MERGE,COMMIT}_PROMPTS.md`
- `./ci.sh`

It also ensures `.gitignore` includes Vizier runtime paths that should stay out of
history:

- `.vizier/tmp/`
- `.vizier/tmp-worktrees/`
- `.vizier/jobs/`
- `.vizier/sessions/`

To validate initialization without mutating files:

```sh
vizier init --check
```

`--check` exits non-zero and prints a missing-item list when marker files or
required ignore rules are absent.

## Dry run

To preview the install actions without writing files:

```sh
./install.sh --dry-run
```

`--dry-run` cannot be combined with `--uninstall`.

## System install

```sh
sudo ./install.sh
```

By default `install.sh` uses `PREFIX=/usr/local`, so a system install typically requires elevated privileges. The script never invokes `sudo` itself.
`install.sh` also checks that the target directories are writable and exits early with guidance if they are not.
When run as root with no explicit `CARGO_TARGET_DIR`, the installer now builds in a temporary directory under `${TMPDIR:-/tmp}` and removes it on exit so it does not leave root-owned `./target` artifacts in the clone.

## Packaging / staging with `DESTDIR`

To stage files into a temporary root (without requiring root permissions):

```sh
DESTDIR="$(mktemp -d)" PREFIX=/usr/local ./install.sh
find "$DESTDIR/usr/local" -maxdepth 4 -type f
```

This writes into:

- `"$DESTDIR/usr/local/bin/vizier"`
- `"$DESTDIR/usr/local/share/vizier/agents/*"`
- `"$DESTDIR$WORKFLOWSDIR/{draft,approve,merge}.toml"`
- `"$DESTDIR/usr/local/share/man/man1/*.1"`
- `"$DESTDIR/usr/local/share/man/man5/*.5"`
- `"$DESTDIR/usr/local/share/man/man7/*.7"`

## Man-page lookup

After install, you can open the shipped pages with standard `man` tooling:

```sh
man vizier
man vizier-jobs
man 5 vizier-config
man 7 vizier-workflow
man 7 vizier-workflow-template
```

If your prefix is not on `MANPATH`, either pass `-M` directly:

```sh
man -M "$HOME/.local/share/man" vizier
```

or prepend `MANPATH`:

```sh
MANPATH="$HOME/.local/share/man:${MANPATH:-}" man vizier-jobs
```

## Directory overrides

`install.sh` supports the usual packaging overrides:

- `CARGO_TARGET_DIR` (default: `target`; root defaults to a temporary target dir unless explicitly set)
- `BINDIR` (default: `"$PREFIX/bin"`)
- `DATADIR` (default: `"$PREFIX/share"`)
- `MANDIR` (default: `"$PREFIX/share/man"`)
- `WORKFLOWSDIR` (default: `<base_config_dir>/vizier/workflows`, where `<base_config_dir>` follows `VIZIER_CONFIG_DIR`, `XDG_CONFIG_HOME`, `APPDATA`, `HOME/.config`, `USERPROFILE/AppData/Roaming`)

Note: Vizier discovers bundled agent shims relative to the binary location:

- `<exe-dir>/agents`
- `<prefix>/share/vizier/agents` (when `vizier` is installed under `<prefix>/bin`)

If you install `vizier` outside the prefix’s `bin/`, either install shims into `<exe-dir>/agents` or set `VIZIER_AGENT_SHIMS_DIR` to the directory that contains `codex/`, `gemini/`, etc.

## Uninstall

If you installed with `install.sh`, you can uninstall using the recorded manifest:

```sh
./install.sh --uninstall
```

For staged installs, pass the same `DESTDIR`/`PREFIX` you used during install.

## Troubleshooting

- `jq not found`: install `jq` (required by `examples/agents/*/filter.sh`).
- `no bundled agent shim named ...`: install the relevant agent CLI (for example `codex`, `gemini`, `claude`) or configure Vizier to use a custom shim via `.vizier/config.toml` / `~/.config/vizier/config.toml`.
- `permission denied`: install into a user prefix (for example `PREFIX="$HOME/.local"`) or rerun the install as root.
- `install destination is not writable`: rerun with `sudo`, set `PREFIX` to a writable directory, or stage with `DESTDIR`.
- `./target` became root-owned after older sudo installs: remove or `chown` it once, then rerun install; current `install.sh` avoids this by using a temporary Cargo target directory when running as root without `CARGO_TARGET_DIR`.

## Development validation

For the repo gate, run:

```sh
./cicd.sh
```

If `CARGO_TARGET_DIR` is unset, the script now defaults to
`.vizier/tmp/cargo-target` so leftover permission-restricted `target/` folders do
not block local gate runs. Set `CARGO_TARGET_DIR` explicitly to override.

The integration fixtures now clean up stale Vizier-owned temp roots before each run.
Cleanup scans `env::temp_dir()` and also `/private/tmp` on macOS to catch legacy
roots created outside user-scoped temp dirs:
`vizier-tests-build-*`, `vizier-tests-repo-*`, legacy `.tmp*` roots that match
the Vizier fixture markers, and legacy `vizier-debug-*` roots that match the old
`repo/` fixture layout. Normal test runs should not leave new temp build roots
behind after process exit.

Integration fixture binaries now reuse the shared Cargo target cache
(`$CARGO_TARGET_DIR` or `.vizier/tmp/cargo-target`) and link into each temp repo, which avoids
rebuilding/copying a full `vizier` binary for every fixture instance.

Fixture repo bootstrap is also cached per test process: Vizier now creates one initialized
template repo and clones from it per test, instead of re-seeding and re-initializing git from
scratch for each integration fixture.

Integration fixtures also prepend local `codex`/`gemini` backend stubs on `PATH`, so test runs
cannot accidentally hit paid external agent binaries if a command resolves through the default
bundled shims.

If you need to inspect integration build artifacts locally, opt into preservation:

```sh
VIZIER_TEST_KEEP_TEMP=1 cargo test -p tests -- --nocapture
```

With `VIZIER_TEST_KEEP_TEMP=1`, fixture build roots are intentionally retained for
debugging and must be removed manually when you are done.

The integration tests isolate their repos and artifacts per test, so default parallel
`cargo test` runs are supported; only set `RUST_TEST_THREADS=1` if you are debugging
ordering-specific failures locally.

Fixture job polling defaults to 50ms. If you need slower polling while debugging on constrained
machines, set `VIZIER_TEST_JOB_POLL_MS`:

```sh
VIZIER_TEST_JOB_POLL_MS=150 cargo test -p tests -- --nocapture
```

If you need to force fixture-level serialization while debugging flakes, set:

```sh
VIZIER_TEST_SERIAL=1 cargo test -p tests -- --nocapture
```

Expected runtime (warm cache): usually tens of seconds on a typical laptop for the integration
suite; cold builds remain dominated by Rust compilation.

Pitfalls we have hit keeping tests stable:
- Background jobs must always finalize with a terminal status. If a job errors before finalization
  (for example a missing agent binary), follow-mode tests can hang waiting for completion.
- Scheduler runs create `.vizier/jobs/` entries; tests that assert a clean worktree should either
  ignore or clean that directory to avoid false dirtiness.
