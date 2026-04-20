#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-}"

if [[ -z "$VERSION" ]]; then
  echo "usage: scripts/release.sh <version>"
  exit 1
fi

TAG="v${VERSION}"
BIN_NAME="ac"
NATIVE_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
INTEL_TARGET="x86_64-apple-darwin"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1"
    exit 1
  fi
}

require_cmd cargo
require_cmd rustup
require_cmd gh
require_cmd tar

cd "$ROOT"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "working tree must be clean before creating a release"
  exit 1
fi

if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "tag already exists: $TAG"
  exit 1
fi

rustup target add "$INTEL_TARGET"

build_target() {
  local target="$1"
  cargo build --release --target "$target"
  local staging_dir="$ROOT/target/release-artifacts/${target}"
  rm -rf "$staging_dir"
  mkdir -p "$staging_dir"
  cp "$ROOT/target/${target}/release/${BIN_NAME}" "$staging_dir/${BIN_NAME}"
  tar -C "$staging_dir" -czf "$ROOT/target/release-artifacts/${BIN_NAME}-${VERSION}-${target}.tar.gz" "$BIN_NAME"
}

mkdir -p "$ROOT/target/release-artifacts"

build_target "$NATIVE_TARGET"
build_target "$INTEL_TARGET"

git tag "$TAG"
git push origin HEAD
git push origin "$TAG"

gh release create "$TAG" \
  "$ROOT/target/release-artifacts/${BIN_NAME}-${VERSION}-${NATIVE_TARGET}.tar.gz" \
  "$ROOT/target/release-artifacts/${BIN_NAME}-${VERSION}-${INTEL_TARGET}.tar.gz" \
  --generate-notes
