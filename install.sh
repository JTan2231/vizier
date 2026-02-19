#!/bin/sh
set -eu

usage() {
  cat <<'EOF'
Usage: ./install.sh [--dry-run] [--uninstall]

Build and install Vizier from a clone.

Environment variables:
  PREFIX   Install prefix (default: /usr/local)
  DESTDIR  Staging root for packaging (default: empty)
  CARGO_TARGET_DIR  Cargo build target dir (default: target; root uses a temp dir)
  BINDIR   Install dir for the vizier binary (default: $PREFIX/bin)
  DATADIR  Install dir for shared data (default: $PREFIX/share)
  MANDIR   Install dir for man pages (default: $PREFIX/share/man)
  WORKFLOWSDIR  Install dir for global workflow templates
               (default: <base_config_dir>/vizier/workflows)

Install layout:
  $BINDIR/vizier
  $DATADIR/vizier/agents/<label>/{agent.sh,filter.sh,...}
  $WORKFLOWSDIR/{draft.hcl,approve.hcl,merge.hcl}
  $MANDIR/man1/*.1
  $MANDIR/man5/*.5
  $MANDIR/man7/*.7

Notes:
  - For system prefixes, run as root or via sudo (this script never invokes sudo).
  - The bundled agent shims' filter scripts require jq.
  - When the repo does not ship Cargo.lock, this script generates it for a
    deterministic build of the current dependency graph.
EOF
}

say() {
  printf '%s\n' "$*"
}

die() {
  printf 'error: %s\n' "$*" 1>&2
  exit 1
}

resolve_base_config_dir() {
  if [ -n "${VIZIER_CONFIG_DIR:-}" ]; then
    printf '%s\n' "$VIZIER_CONFIG_DIR"
    return 0
  fi
  if [ -n "${XDG_CONFIG_HOME:-}" ]; then
    printf '%s\n' "$XDG_CONFIG_HOME"
    return 0
  fi
  if [ -n "${APPDATA:-}" ]; then
    printf '%s\n' "$APPDATA"
    return 0
  fi
  if [ -n "${HOME:-}" ]; then
    printf '%s\n' "$HOME/.config"
    return 0
  fi
  if [ -n "${USERPROFILE:-}" ]; then
    printf '%s\n' "$USERPROFILE/AppData/Roaming"
    return 0
  fi
  return 1
}

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
cd "$script_dir"

dry_run=0
uninstall=0

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run)
      dry_run=1
      ;;
    --uninstall)
      uninstall=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
  shift
done

if [ "$dry_run" -eq 1 ] && [ "$uninstall" -eq 1 ]; then
  die "--dry-run and --uninstall cannot be combined"
fi

PREFIX=${PREFIX:-/usr/local}
DESTDIR=${DESTDIR:-}
BINDIR=${BINDIR:-"$PREFIX/bin"}
DATADIR=${DATADIR:-"$PREFIX/share"}
MANDIR=${MANDIR:-"$PREFIX/share/man"}
if base_config_dir=$(resolve_base_config_dir); then
  default_workflows_dir="$base_config_dir/vizier/workflows"
else
  default_workflows_dir=""
fi
WORKFLOWSDIR=${WORKFLOWSDIR:-"$default_workflows_dir"}
if [ -z "$WORKFLOWSDIR" ]; then
  die "unable to resolve WORKFLOWSDIR default (set WORKFLOWSDIR or one of VIZIER_CONFIG_DIR/XDG_CONFIG_HOME/APPDATA/HOME/USERPROFILE)"
fi

agents_src="examples/agents"
man_src_root="docs/man"
workflow_src_root=".vizier/workflows"
workflow_seed_files="draft.hcl approve.hcl merge.hcl"

manifest_rel="$DATADIR/vizier/install-manifest.txt"
manifest_path="$DESTDIR$manifest_rel"
temp_target_dir=""

is_writable_parent() {
  target="$1"
  dir="$target"

  while [ ! -d "$dir" ]; do
    parent=$(dirname "$dir")
    if [ "$parent" = "$dir" ]; then
      break
    fi
    dir="$parent"
  done

  [ -w "$dir" ]
}

check_install_permissions() {
  if [ "$dry_run" -eq 1 ] || [ "$uninstall" -eq 1 ]; then
    return 0
  fi

  if [ "$(id -u)" -eq 0 ]; then
    return 0
  fi

  unwritable=""
  for path in "$DESTDIR$BINDIR" "$DESTDIR$DATADIR" "$DESTDIR$MANDIR" "$DESTDIR$WORKFLOWSDIR"; do
    if ! is_writable_parent "$path"; then
      unwritable="${unwritable}  $path\n"
    fi
  done

  if [ -n "$unwritable" ]; then
    {
      printf 'error: install destination is not writable\n'
      printf 'The following paths are not writable by the current user:\n'
      printf '%b' "$unwritable"
      printf '\nTry one of:\n'
      printf '  - sudo ./install.sh\n'
      printf '  - PREFIX="$HOME/.local" ./install.sh\n'
      printf '  - DESTDIR=/path/to/stage PREFIX=/usr/local ./install.sh\n'
    } 1>&2
    exit 1
  fi
}

run() {
  if [ "$dry_run" -eq 1 ]; then
    say "+ $*"
    return 0
  fi
  "$@"
}

setup_cargo_target_dir() {
  target_dir=${CARGO_TARGET_DIR:-target}

  if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    return 0
  fi

  if [ "$(id -u)" -eq 0 ]; then
    if [ "$dry_run" -eq 1 ]; then
      target_dir="${TMPDIR:-/tmp}/vizier-install-target.XXXXXX"
      return 0
    fi

    temp_target_dir=$(mktemp -d "${TMPDIR:-/tmp}/vizier-install-target.XXXXXX")
    target_dir="$temp_target_dir"
    export CARGO_TARGET_DIR="$target_dir"
  fi
}

cleanup_temp_target_dir() {
  if [ -n "$temp_target_dir" ] && [ -d "$temp_target_dir" ]; then
    rm -rf "$temp_target_dir"
  fi
}

trap cleanup_temp_target_dir EXIT

install_dir() {
  run install -d "$1"
}

install_file() {
  mode="$1"
  src="$2"
  dst="$3"

  install_dir "$(dirname "$dst")"
  run install -m "$mode" "$src" "$dst"
}

if [ "$uninstall" -eq 1 ]; then
  if [ ! -f "$manifest_path" ]; then
    die "manifest not found: $manifest_path"
  fi

  if [ "$dry_run" -eq 1 ]; then
    say "+ rm -f <paths from $manifest_path>"
    say "+ rm -f $manifest_path"
    exit 0
  fi

  while IFS= read -r relpath; do
    [ -n "$relpath" ] || continue
    rm -f "$DESTDIR$relpath"
  done <"$manifest_path"

  rm -f "$manifest_path"

  rmdir "$DESTDIR$DATADIR/vizier/agents/codex" 2>/dev/null || true
  rmdir "$DESTDIR$DATADIR/vizier/agents/gemini" 2>/dev/null || true
  rmdir "$DESTDIR$DATADIR/vizier/agents/claude" 2>/dev/null || true
  rmdir "$DESTDIR$DATADIR/vizier/agents" 2>/dev/null || true
  rmdir "$DESTDIR$DATADIR/vizier" 2>/dev/null || true
  rmdir "$DESTDIR$WORKFLOWSDIR" 2>/dev/null || true

  say "Uninstalled files listed in $manifest_rel"
  exit 0
fi

if [ ! -d "$agents_src" ]; then
  die "missing agent shims directory: $agents_src"
fi

if [ ! -d "$man_src_root" ]; then
  die "missing man pages directory: $man_src_root"
fi

if [ ! -d "$workflow_src_root" ]; then
  die "missing workflow templates directory: $workflow_src_root"
fi

for workflow_file in $workflow_seed_files; do
  if [ ! -f "$workflow_src_root/$workflow_file" ]; then
    die "missing workflow seed template: $workflow_src_root/$workflow_file"
  fi
done

man_files=$(find "$man_src_root" -type f -path "$man_src_root/man*/*" | sort)
if [ -z "$man_files" ]; then
  die "no man pages found under $man_src_root/man*/"
fi

check_install_permissions

setup_cargo_target_dir
bin_src="$target_dir/release/vizier"

if [ "$dry_run" -eq 1 ]; then
  say "+ cargo generate-lockfile (if Cargo.lock missing)"
  say "+ cargo build --locked --release -p vizier"
else
  if [ ! -f Cargo.lock ]; then
    say "Generating Cargo.lock for a deterministic build..." 1>&2
    cargo generate-lockfile
  fi
  cargo build --locked --release -p vizier
fi

if [ "$dry_run" -eq 0 ] && [ ! -f "$bin_src" ]; then
  die "built binary not found: $bin_src (set CARGO_TARGET_DIR or build manually)"
fi

installed_paths=""
installed_man_targets=""
installed_workflow_targets=""
retained_workflow_targets=""
skipped_workflow_targets=""

record_manifest_path() {
  installed_paths="${installed_paths}${1}\n"
}

install_file 0755 "$bin_src" "$DESTDIR$BINDIR/vizier"
record_manifest_path "$BINDIR/vizier"

for src in $(find "$agents_src" -type f); do
  rel=${src#"$agents_src"/}
  dst_rel="$DATADIR/vizier/agents/$rel"
  install_file 0755 "$src" "$DESTDIR$dst_rel"
  record_manifest_path "$dst_rel"
done

for src in $man_files; do
  rel=${src#"$man_src_root"/}
  dst_rel="$MANDIR/$rel"
  install_file 0644 "$src" "$DESTDIR$dst_rel"
  record_manifest_path "$dst_rel"
  installed_man_targets="${installed_man_targets}  $dst_rel\n"
done

for workflow_file in $workflow_seed_files; do
  src="$workflow_src_root/$workflow_file"
  dst_rel="$WORKFLOWSDIR/$workflow_file"
  dst="$DESTDIR$dst_rel"

  if [ -e "$dst" ]; then
    if [ -f "$dst" ] && cmp -s "$src" "$dst"; then
      record_manifest_path "$dst_rel"
      retained_workflow_targets="${retained_workflow_targets}  $dst_rel\n"
    else
      skipped_workflow_targets="${skipped_workflow_targets}  $dst_rel\n"
    fi
    continue
  fi

  install_file 0644 "$src" "$dst"
  record_manifest_path "$dst_rel"
  installed_workflow_targets="${installed_workflow_targets}  $dst_rel\n"
done

install_dir "$(dirname "$manifest_path")"
if [ "$dry_run" -eq 1 ]; then
  say "+ write manifest $manifest_rel"
else
  printf "%b" "$installed_paths" | sort -u >"$manifest_path"
fi

say "Installed:"
say "  $BINDIR/vizier"
say "  $DATADIR/vizier/agents/*"
if [ -n "$installed_workflow_targets" ]; then
  say "Global workflow templates (installed):"
  printf "%b" "$installed_workflow_targets"
fi
if [ -n "$retained_workflow_targets" ]; then
  say "Global workflow templates (retained unchanged):"
  printf "%b" "$retained_workflow_targets"
fi
if [ -n "$skipped_workflow_targets" ]; then
  say "Global workflow templates (preserved existing):"
  printf "%b" "$skipped_workflow_targets"
fi
say "Man pages:"
printf "%b" "$installed_man_targets"
say "Manifest:"
say "  $manifest_rel"
