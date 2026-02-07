# Installing Vizier (from a clone)

Vizier can be installed directly from this repository. The installer builds the `vizier` binary and installs the bundled agent shim scripts and the `vizier(1)` man page into a standard prefix layout.

## Prerequisites

- Rust toolchain (`cargo`)
- `git`
- `jq` (recommended): required by the bundled `filter.sh` scripts used to render agent JSONL progress

## Quick start (user prefix)

```sh
PREFIX="$HOME/.local" ./install.sh
```

This installs:

- `"$HOME/.local/bin/vizier"`
- `"$HOME/.local/share/vizier/agents/*"`
- `"$HOME/.local/share/man/man1/vizier.1"`

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

## Packaging / staging with `DESTDIR`

To stage files into a temporary root (without requiring root permissions):

```sh
DESTDIR="$(mktemp -d)" PREFIX=/usr/local ./install.sh
find "$DESTDIR/usr/local" -maxdepth 4 -type f
```

This writes into:

- `"$DESTDIR/usr/local/bin/vizier"`
- `"$DESTDIR/usr/local/share/vizier/agents/*"`
- `"$DESTDIR/usr/local/share/man/man1/vizier.1"`

## Directory overrides

`install.sh` supports the usual packaging overrides:

- `BINDIR` (default: `"$PREFIX/bin"`)
- `DATADIR` (default: `"$PREFIX/share"`)
- `MANDIR` (default: `"$PREFIX/share/man"`)

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

## Development validation

For the repo gate, run:

```sh
./cicd.sh
```

If `CARGO_TARGET_DIR` is unset, the script now defaults to
`.vizier/tmp/cargo-target` so leftover permission-restricted `target/` folders do
not block local gate runs. Set `CARGO_TARGET_DIR` explicitly to override.

The integration fixtures now clean up stale Vizier-owned temp roots before each run:
`vizier-tests-build-*`, `vizier-tests-repo-*`, and legacy `.tmp*` roots that match
the Vizier fixture markers. Normal test runs should not leave new temp build roots
behind after process exit.

If you need to inspect integration build artifacts locally, opt into preservation:

```sh
VIZIER_TEST_KEEP_TEMP=1 cargo test -p tests -- --nocapture
```

With `VIZIER_TEST_KEEP_TEMP=1`, fixture build roots are intentionally retained for
debugging and must be removed manually when you are done.

The integration tests isolate their repos and artifacts per test, so default parallel
`cargo test` runs are supported; only set `RUST_TEST_THREADS=1` if you are debugging
ordering-specific failures locally.

Expected runtime: plan on ~1-2 minutes on a typical laptop (Rust build + 100+ integration tests).

Pitfalls we have hit keeping tests stable:
- Background jobs must always finalize with a terminal status. If a job errors before finalization
  (for example a missing agent binary), follow-mode tests can hang waiting for completion.
- Scheduler runs create `.vizier/jobs/` entries; tests that assert a clean worktree should either
  ignore or clean that directory to avoid false dirtiness.
