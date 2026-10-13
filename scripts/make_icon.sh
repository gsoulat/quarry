#!/usr/bin/env bash
# Génère les icônes de Quarry à partir de assets/icon.svg (macOS uniquement :
# le rendu passe par Quick Look et l'icns par iconutil) :
#   assets/icon.png      1024 px, icône du Dock
#   assets/icon-256.png  256 px, icône de fenêtre (incluse dans le binaire)
#   assets/icon.icns     jeu complet pour le bundle macOS
set -euo pipefail
cd "$(dirname "$0")/.."
svg=assets/icon.svg
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

render() { # render <taille> <sortie>
  qlmanage -t -s "$1" -o "$tmp" "$svg" >/dev/null 2>&1
  mv "$tmp/icon.svg.png" "$2"
}

iconset="$tmp/icon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  render "$size" "$iconset/icon_${size}x${size}.png"
  render "$((size * 2))" "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o assets/icon.icns
cp "$iconset/icon_512x512@2x.png" assets/icon.png
cp "$iconset/icon_256x256.png" assets/icon-256.png
ls -la assets/icon.png assets/icon-256.png assets/icon.icns
