# Contributor: PocketModem Team <pocketmodem@example.org>
# Maintainer: PocketModem Team <pocketmodem@example.org>
#
# APK Build for PocketModem on PostmarketOS
# Build in native aarch64 chroot (no cross-compilation)
#
# Build dependencies on pmOS host:
#   apk add cargo rust rust-std-aarch64-alpine-linux-musl \
#            pkgconf gtk4.0-dev libadwaita-dev libudev-dev

pkgname=pocket-modem
pkgver=0.1.0
pkgrel=0
pkgdesc="GTK4 UI for PocketModem Radio with native KV4P protocol"
url="https://github.com/pocketmodem/pocket-modem"
arch="aarch64"
license="GPL-3.0-or-later"
options="!check"
makedepends="
    cargo
    rust
    rust-std-aarch64-alpine-linux-musl
    pkgconf
    gtk4.0-dev
    libadwaita-dev
    libudev-dev
    "
depends="gtk4.0 libudev"

build() {
    cd "$srcdir/pocket-modem"
    cargo build --release
}

package() {
    cd "$srcdir/pocket-modem"
    install -Dm755 target/release/pocket-modem \
        "$pkgdir"/usr/bin/pocket-modem
    install -Dm644 icon.png \
        "$pkgdir"/usr/share/pixmaps/pocket-modem.png
    install -Dm644 org.pocketmodem.pocket-modem.desktop \
        "$pkgdir"/usr/share/applications/org.pocketmodem.pocket-modem.desktop
    # Symlink for app menu (Phosh)
    install -d "$pkgdir"/usr/share/applications
    ln -sf ../applications/org.pocketmodem.pocket-modem.desktop \
        "$pkgdir"/usr/share/applications/pocket-modem.desktop
    # Create icon theme directory for GTK
    install -d "$pkgdir"/usr/share/icons/hicolor/128x128/apps
    install -Dm644 icon.png \
        "$pkgdir"/usr/share/icons/hicolor/128x128/apps/pocket-modem.png
}

sha512sums="SKIP"