# `vizier release`

`vizier release` creates a local release event from Git history.

It is intentionally Git-native for MVP:
- version source of truth is tags named `v<major>.<minor>.<patch>`
- bump detection uses Conventional Commits since the last reachable release tag
- release notes are generated from those commits
- release creates an allow-empty release commit and (by default) an annotated tag
- no remote publish APIs are called

## Usage

```bash
vizier release [--dry-run] [--yes] [--major|--minor|--patch] [--max-commits N] [--no-tag] [--release-script <cmd> | --no-release-script]
```

Release-script precedence for a given invocation:

1. `--no-release-script` disables release script execution.
2. `--release-script <cmd>` overrides config for that run.
3. `[release.gate].script` is used when configured.
4. If none of the above applies, no release script runs.

Configure a default release gate script:

```toml
[release.gate]
script = "./cicd.sh"
```

## Bump policy

Auto bump precedence:
1. `major`: any `BREAKING CHANGE` footer or `type!:` subject marker
2. `minor`: any `feat` commit
3. `patch`: any `fix` or `perf` commit
4. `none`: no releasable commits

If bump resolves to `none`, Vizier prints a no-op outcome unless you force `--patch`, `--minor`, or `--major`.

## Notes policy

Release notes render a single section:
- `Changes`

Only commits with Conventional Commit subject format are included:
- `type: description`
- `type(scope): description`
- `type!: description`
- `type(scope)!: description`

Where `type` is alphabetic and `description` is non-empty.

The `Changes` section is capped by `--max-commits` (default `20`); overflow is summarized as `+N more`.

## Safety checks

Release refuses to mutate history when:
- working tree is dirty (excluding ephemeral `.vizier/tmp*`, `.vizier/jobs`, `.vizier/sessions`)
- Git is mid merge/cherry-pick/rebase/revert/bisect
- `HEAD` is detached
- the target release tag already exists

## Release gate execution and rollback

When a release script is configured/resolved for a non-dry-run release:

1. Vizier creates the release commit.
2. Vizier creates the release tag (unless `--no-tag`).
3. Vizier runs the release script from the repository root.

The release script receives:

- `VIZIER_RELEASE_VERSION` (for example `0.4.2`)
- `VIZIER_RELEASE_TAG` (for example `v0.4.2`; empty when `--no-tag`)
- `VIZIER_RELEASE_COMMIT` (release commit SHA)
- `VIZIER_RELEASE_RANGE` (for example `v0.4.1..HEAD`)

If the script exits non-zero or fails to launch, Vizier treats the release as failed and attempts local rollback:

- delete the created release tag (if one was created),
- reset the original branch back to its start commit,
- restore index/worktree to that start commit.

Rollback applies only to local Git state. External side effects from the script are not reverted.

`vizier release --dry-run` never creates commit/tag state and never runs release scripts.

## Examples

Preview only:

```bash
vizier release --dry-run
```

Create commit + tag without prompt:

```bash
vizier release --yes
```

Force a minor bump and skip tag creation:

```bash
vizier release --yes --minor --no-tag
```

Override the configured release script for one run:

```bash
vizier release --yes --release-script "./scripts/publish.sh --channel stable"
```

Disable the configured release script for one run:

```bash
vizier release --yes --no-release-script
```
