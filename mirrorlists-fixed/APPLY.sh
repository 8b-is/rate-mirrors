#!/usr/bin/env bash
# Apply cleaned mirrorlists to /etc/pacman.d and refresh pacman DBs.
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
ts=$(date +%Y%m%d%H%M%S)

for f in mirrorlist cachyos-mirrorlist cachyos-v3-mirrorlist cachyos-v4-mirrorlist; do
  [[ -f "$DIR/$f" ]] || { echo "missing $DIR/$f" >&2; exit 1; }
done

echo "==> Backing up current lists to *.pre-fix.$ts"
for f in mirrorlist cachyos-mirrorlist cachyos-v3-mirrorlist cachyos-v4-mirrorlist; do
  sudo cp -a "/etc/pacman.d/$f" "/etc/pacman.d/${f}.pre-fix.$ts"
  sudo install -m644 "$DIR/$f" "/etc/pacman.d/$f"
  echo "    installed /etc/pacman.d/$f ($(grep -c '^Server' "/etc/pacman.d/$f") servers)"
done

echo "==> pacman -Syy"
sudo pacman -Syy

echo "==> Sample package URLs (bash):"
sudo pacman -Sp bash | head -10

echo
echo "Done. If anything looks wrong:"
echo "  sudo cp -a /etc/pacman.d/mirrorlist.pre-fix.$ts /etc/pacman.d/mirrorlist"
echo "  # same for cachyos-*"
