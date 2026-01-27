#!/bin/sh
set -eu

usage() {
  cat <<'EOF'
Usage: ./install.sh [--dry-run] [--uninstall]

<<<<<<< HEAD
Build and install Vizier from a clone.

Environment variables:
  PREFIX   Install prefix (default: /usr/local)
  DESTDIR  Staging root for packaging (default: empty)
  BINDIR   Install dir for the vizier binary (default: $PREFIX/bin)
  DATADIR  Install dir for shared data (default: $PREFIX/share)
  MANDIR   Install dir for man pages (default: $PREFIX/share/man)

Install layout:
  $BINDIR/vizier
  $DATADIR/vizier/agents/<label>/{agent.sh,filter.sh,...}
  $MANDIR/man1/vizier.1

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

agents_src="examples/agents"
man_src="docs/man/man1/vizier.1"

manifest_rel="$DATADIR/vizier/install-manifest.txt"
manifest_path="$DESTDIR$manifest_rel"

run() {
  if [ "$dry_run" -eq 1 ]; then
    say "+ $*"
    return 0
  fi
  "$@"
}

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

  say "Uninstalled files listed in $manifest_rel"
  exit 0
fi

if [ ! -d "$agents_src" ]; then
  die "missing agent shims directory: $agents_src"
fi

if [ ! -f "$man_src" ]; then
  die "missing man page: $man_src"
fi

target_dir=${CARGO_TARGET_DIR:-target}
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

install_file 0644 "$man_src" "$DESTDIR$MANDIR/man1/vizier.1"
record_manifest_path "$MANDIR/man1/vizier.1"

install_dir "$(dirname "$manifest_path")"
if [ "$dry_run" -eq 1 ]; then
  say "+ write manifest $manifest_rel"
else
  printf "%b" "$installed_paths" | sort -u >"$manifest_path"
fi

say "Installed:"
say "  $BINDIR/vizier"
say "  $DATADIR/vizier/agents/*"
say "  $MANDIR/man1/vizier.1"
say "Manifest:"
say "  $manifest_rel"
