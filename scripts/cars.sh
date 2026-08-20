#!/usr/bin/env bash
# Compile the cars, and say which source models nobody has compiled yet.
#
# Every config in assets/configs/ names the model it is built from, because the names do not match
# — bmw_e36.toml comes from bmw_3-series_e36.glb — and the models are not in git, so nothing else
# records the pairing. That one line is what lets this script answer both of the questions anyone
# adding a car has: what is already done, and what is sitting in assets/source/ waiting.
#
# Usage:
#   scripts/cars.sh list              what is compiled, and what has no config yet
#   scripts/cars.sh build             compile every configured car
#   scripts/cars.sh build bmw_e36 ... compile only these
#
# Conversion takes about a minute a car and prints a report worth reading; `list` costs nothing.

set -euo pipefail
cd "$(dirname "$0")/.."

CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
SOURCES=assets/source
CONFIGS=assets/configs
COMPILED=assets/compiled

# The `source = "..."` line out of a config, or empty if it has none.
source_of() {
    sed -n 's/^source *= *"\(.*\)"/\1/p' "$1" | head -1
}

list() {
    printf '%-16s %-52s %s\n' CAR MODEL COMPILED
    local claimed=()
    for config in "$CONFIGS"/*.toml; do
        local name src state
        name=$(basename "$config" .toml)
        src=$(source_of "$config")
        claimed+=("$src")
        if [ -f "$COMPILED/$name.azcar" ]; then
            state="$(( $(stat -c%s "$COMPILED/$name.azcar") / 1024 )) KB"
        else
            state="not built"
        fi
        if [ -n "$src" ] && [ ! -f "$SOURCES/$src" ]; then
            state="$state (model missing)"
        fi
        printf '%-16s %-52s %s\n' "$name" "${src:-<unset>}" "$state"
    done

    # Anything in the source folder that no config claims is a car waiting to be added. This is the
    # line the add-car skill starts from.
    local new=()
    for model in "$SOURCES"/*.glb; do
        [ -e "$model" ] || continue
        local base found
        base=$(basename "$model")
        found=no
        for c in "${claimed[@]:-}"; do
            [ "$c" = "$base" ] && found=yes && break
        done
        [ "$found" = no ] && new+=("$base")
    done

    echo
    if [ ${#new[@]} -eq 0 ]; then
        echo "No unconverted models in $SOURCES."
    else
        echo "${#new[@]} model(s) in $SOURCES with no config yet:"
        printf '  %s\n' "${new[@]}"
    fi
}

build() {
    local names=("$@")
    if [ ${#names[@]} -eq 0 ]; then
        for config in "$CONFIGS"/*.toml; do names+=("$(basename "$config" .toml)"); done
    fi
    for name in "${names[@]}"; do
        local config="$CONFIGS/$name.toml"
        [ -f "$config" ] || { echo "no such config: $config" >&2; exit 1; }
        local src
        src=$(source_of "$config")
        [ -n "$src" ] || { echo "$config has no source = line; add one" >&2; exit 1; }
        [ -f "$SOURCES/$src" ] || { echo "$config names $src, which is not in $SOURCES" >&2; exit 1; }
        echo "=== $name  <- $src"
        "$CARGO" run --release -q -p anglezero-asset --bin anglezero-asset -- \
            convert "$SOURCES/$src" "$COMPILED/$name.azcar" --config "$config"
    done
}

case "${1:-list}" in
    list) list ;;
    build) shift; build "$@" ;;
    *) sed -n '2,14p' "$0" >&2; exit 1 ;;
esac
