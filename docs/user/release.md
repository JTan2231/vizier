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
vizier release [--dry-run] [--yes] [--major|--minor|--patch] [--max-commits N] [--no-tag]
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
