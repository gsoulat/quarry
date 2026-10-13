#!/usr/bin/env bash
# Appelé par semantic-release (plugin exec) pendant l'étape « prepare » :
# écrit la version calculée dans Cargo.toml et resynchronise Cargo.lock
# pour le paquet du dépôt, sans toucher aux dépendances.
set -euo pipefail
version="$1"
perl -0pi -e 's/^version = "[^"]+"/version = "'"$version"'"/m' Cargo.toml
cargo update --workspace
grep -q "^version = \"$version\"" Cargo.toml
