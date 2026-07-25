# Maintainer: 8b-is <ca@8b.is>
# Contributor: Nikita Almakov <nikita.almakov@gmail.com> (original author of rate-mirrors)

pkgname=rate-mirrors
pkgver=0.30.0
pkgrel=1
pkgdesc="Map-aware mirror ranking tool with pinnable mirror sources and mirror cross-verification"
arch=('x86_64' 'aarch64')
url="https://github.com/8b-is/rate-mirrors"
license=('CC-BY-NC-SA-3.0')
depends=('gcc-libs')
makedepends=('cargo')
# Drop-in replacement for the upstream package: same binary name and CLI, so
# cachyos-rate-mirrors and any other wrapper keep working unchanged.
provides=('rate-mirrors')
conflicts=('rate-mirrors' 'rate-mirrors-bin' 'rate-mirrors-git')
source=("$pkgname-$pkgver.tar.gz::$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')

prepare() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR=target
  cargo build --frozen --release --all-features
}

check() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  # Unit tests only; nothing here reaches the network.
  cargo test --frozen --release
}

package() {
  cd "$pkgname-$pkgver"

  # The crate is rate_mirrors; the command everyone calls is rate-mirrors.
  install -Dm0755 "target/release/rate_mirrors" "$pkgdir/usr/bin/rate-mirrors"

  install -Dm0644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm0644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
  install -Dm0644 CHANGELOG.md "$pkgdir/usr/share/doc/$pkgname/CHANGELOG.md"

  # Where an administrator drops a pinned mirror list. Sources are looked up here
  # before anything is fetched over the network; an empty directory simply means
  # the published list is used.
  install -dm0755 "$pkgdir/etc/rate-mirrors/sources"
}
