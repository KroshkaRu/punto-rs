#!/usr/bin/env bash
set -euo pipefail
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary=${1:-"$project/target/release/punto-rs"}
test -x "$binary" || { printf '%s\n' 'Сначала соберите release-бинарник на Ubuntu 22.04.' >&2; exit 1; }
version=$("$binary" --version | cut -d ' ' -f 2)
arch=$(dpkg --print-architecture)
stage=$(mktemp -d /tmp/punto-deb.XXXXXX)
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/share/ibus/component" "$stage/usr/share/applications" "$stage/usr/share/icons/hicolor/scalable/apps" "$stage/usr/share/punto-rs/scripts" "$stage/usr/share/doc/punto-rs" "$project/dist"
install -m 755 "$binary" "$stage/usr/bin/punto-rs"
sed 's|@BINARY@|/usr/bin/punto-rs|g' "$project/packaging/punto-rs.xml.in" > "$stage/usr/share/ibus/component/punto-rs.xml"
sed 's|@BINARY@|/usr/bin/punto-rs|g' "$project/packaging/punto-rs.desktop.in" > "$stage/usr/share/applications/punto-rs.desktop"
install -m 644 "$project/packaging/punto-rs.svg" "$stage/usr/share/icons/hicolor/scalable/apps/punto-rs.svg"
install -m 755 "$project/scripts/install.sh" "$stage/usr/share/punto-rs/scripts/install.sh"
cp -R "$project/gnome-extension" "$stage/usr/share/punto-rs/gnome-extension"
install -m 644 "$project/README.md" "$project/LICENSE" "$stage/usr/share/doc/punto-rs/"
cp -R "$project/docs" "$stage/usr/share/doc/punto-rs/"
install -m 644 "$project/assets/material-icons/LICENSE" "$stage/usr/share/doc/punto-rs/material-icons-LICENSE"
install -m 644 "$project/assets/material-icons/NOTICE" "$stage/usr/share/doc/punto-rs/material-icons-NOTICE"
install -m 644 "$project/assets/licenses/pulseaudio-LICENSE" "$stage/usr/share/doc/punto-rs/"
cat > "$stage/DEBIAN/control" <<EOF
Package: punto-rs
Version: $version
Architecture: $arch
Maintainer: Punto RS contributors <maintainers@punto-rs.local>
Depends: libc6 (>= 2.35), libgcc-s1, at-spi2-core, hunspell-ru, hunspell-en-us, libx11-6, libxcb1, libxkbcommon0, libxkbcommon-x11-0, libegl1, libgl1, libxi6, libxrandr2, libxcursor1, libxinerama1
Suggests: ibus (>= 1.5.26), ibus-gtk3, gnome-shell-extension-appindicator
Section: utils
Priority: optional
Description: Automatic Russian and English keyboard layout switcher
 Native AT-SPI layout correction with X11 or portal shortcuts, optional IBus,
 selection conversion, verified undo, application exceptions and a tray.
EOF
dpkg-deb --root-owner-group --build "$stage" "$project/dist/punto-rs_${version}_${arch}.deb"
(
    cd "$project/dist"
    sha256sum "punto-rs_${version}_${arch}.deb" > SHA256SUMS
)
