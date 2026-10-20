#!/usr/bin/env bash
set -euo pipefail
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
if [[ ${1:-} == --help ]]; then
    printf '%s\n' 'Использование: scripts/build-deb.sh [--native]' \
        'Без параметров: сборка в Ubuntu 22.04 через Docker для Ubuntu 22.04–26.04.' \
        '--native: сборка установленным Rust для текущей системы.'
    exit 0
fi
if [[ $# -gt 1 || ( $# -eq 1 && $1 != --native ) ]]; then
    printf '%s\n' 'Неизвестный параметр; см. scripts/build-deb.sh --help' >&2
    exit 2
fi
command -v dpkg-deb >/dev/null || { printf '%s\n' 'Установите dpkg-dev.' >&2; exit 1; }
if [[ ${1:-} == --native ]]; then
    command -v cargo >/dev/null || { printf '%s\n' 'Нужен Rust 1.88+: https://rustup.rs' >&2; exit 1; }
    CARGO_TARGET_DIR="$project/target/native" cargo build --manifest-path "$project/Cargo.toml" --release --locked
    binary="$project/target/native/release/punto-rs"
else
    command -v docker >/dev/null || { printf '%s\n' 'Нужен Docker; альтернативно: scripts/build-deb.sh --native' >&2; exit 1; }
    docker build --network host -t punto-rs-builder:ubuntu22.04 -f "$project/packaging/build.Dockerfile" "$project"
    mkdir -p "$project/target/ubuntu22" "$project/target/build-cache/registry" "$project/target/build-cache/git"
    # Sources stay read-only; Cargo caches and artifacts remain owned by the caller.
    docker run --rm --init --network host --user "$(id -u):$(id -g)" \
        -e HOME=/tmp -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/build \
        -v "$project:/work:ro" -v "$project/target/ubuntu22:/build" \
        -v "$project/target/build-cache/registry:/tmp/cargo/registry" \
        -v "$project/target/build-cache/git:/tmp/cargo/git" \
        punto-rs-builder:ubuntu22.04 cargo build --release --locked
    binary="$project/target/ubuntu22/release/punto-rs"
fi
"$project/scripts/package-deb.sh" "$binary"
