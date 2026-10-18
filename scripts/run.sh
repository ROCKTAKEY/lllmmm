#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

usage() {
    printf '%s\n' \
        'Usage: scripts/run.sh [CONFIG_FILE [serve|check]]' \
        'Default: config/lllmmm.toml in the repository; serve in the foreground.' \
        'Builds with Guix, then starts lllmmm with its runtime dependencies.'
}

case "${1:-}" in
    -h|--help) usage; exit 0 ;;
esac
if [ "$#" -gt 2 ]; then
    usage >&2
    exit 2
fi
config_file=${1:-"$project_dir/config/lllmmm.toml"}
action=${2:-serve}
case "$action" in
    serve|check) ;;
    *) usage >&2; exit 2 ;;
esac
case "$config_file" in
    /*) ;;
    *) config_file="$PWD/$config_file" ;;
esac
if [ ! -f "$config_file" ]; then
    printf 'Configuration not found: %s\n' "$config_file" >&2
    printf 'From the repository, run: cp -r examples config\nThen edit config/lllmmm.toml and config/strata/*.json for your installation.\n' >&2
    exit 2
fi
if ! command -v guix >/dev/null 2>&1; then
    printf '%s\n' 'guix is required on PATH.' >&2
    exit 127
fi

cd -- "$project_dir"
guix shell -m "$project_dir/manifest.scm" -CWNF -- \
    cargo build --locked --release --target-dir "$project_dir/target"

# The runtime needs the host's model directories and GPU devices.
# Terminal Ctrl-C reaches Guix and lllmmm in the foreground process group.
exec guix shell -m "$project_dir/runtime-manifest.scm" -- \
    "$project_dir/target/release/lllmmm" --config "$config_file" "$action"
