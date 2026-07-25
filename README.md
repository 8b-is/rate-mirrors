# Rate Mirrors

[![License: CC BY-NC-SA 3.0](https://img.shields.io/badge/License-CC%20BY--NC--SA%203.0-lightgrey.svg)](https://creativecommons.org/licenses/by-nc-sa/3.0/)

A fast mirror ranking tool for Arch-based distributions. It uses submarine cable and
internet exchange data to hop between countries and find genuinely fast mirrors in
about 30 seconds — then checks that the mirrors it picked are serving the same
repository everyone else is.

Maintained by [8b-is](https://github.com/8b-is/rate-mirrors).
Current version: 0.30.0

## Table of Contents

- [Quick Start](#quick-start)
- [Installation](#installation)
- [Supported Distributions](#supported-distributions)
- [Where mirror lists come from](#where-mirror-lists-come-from)
- [Mirror verification](#mirror-verification)
- [Choosing which mirrors you will accept](#choosing-which-mirrors-you-will-accept)
- [Options](#options)
- [Algorithm](#algorithm)
- [Examples](#examples)
- [Exit Codes](#exit-codes)
- [Credits and License](#credits-and-license)

## Quick Start

```bash
# Arch Linux
rate-mirrors arch | sudo tee /etc/pacman.d/mirrorlist

# CachyOS
rate-mirrors cachyos | sudo tee /etc/pacman.d/cachyos-mirrorlist

# Skip mirrors in countries you would rather not pull packages from
rate-mirrors --exclude-countries=RU,CN arch | sudo tee /etc/pacman.d/mirrorlist

# With a backup
export TMPFILE="$(mktemp)"; \
    rate-mirrors --save=$TMPFILE arch --max-delay=21600 \
    && sudo mv /etc/pacman.d/mirrorlist /etc/pacman.d/mirrorlist-backup \
    && sudo mv $TMPFILE /etc/pacman.d/mirrorlist

# See all options
rate-mirrors --help
```

No configuration is required. Every subcommand works on a fresh system with no
flags and no files to set up first.

## Installation

| Method | Command |
|--------|---------|
| Arch package (this fork) | `makepkg -si` from the included [`PKGBUILD`](PKGBUILD) |
| From a git checkout | `./install.sh --system` |
| From source | `cargo build --release --locked` |

The package `provides`/`conflicts` `rate-mirrors`, installs the same
`/usr/bin/rate-mirrors`, and keeps the same command-line interface — so
`cachyos-rate-mirrors` and any other wrapper keep working unchanged.

### Install from a git checkout

```bash
# One-shot on CachyOS: build → install to /usr/bin → smoke test →
# pacman -Syu → reinstall (if the package overwrote us) → rank mirrors
./install.sh --all

# Unattended
./install.sh --all --noconfirm

# Exclude extra countries
./install.sh --all --exclude-countries=RU,CN

# Install only
./install.sh --system --smoke

# Re-rank only, after a prior install
./install.sh --rank-only --exclude-countries=RU
```

Until this fork is packaged by your distribution, a `pacman -Syu` that ships the
upstream `rate-mirrors` package will replace `/usr/bin/rate-mirrors`. Re-run
`./install.sh --system` afterwards, or build the included `PKGBUILD` so pacman
tracks this version as the installed one.

## Supported Distributions

### Arch-based

| Command | Distribution | Notes |
|---------|-------------|-------|
| `rate-mirrors arch` | Arch Linux | Skips outdated/syncing mirrors |
| `rate-mirrors arch4edu` | Arch4edu | |
| `rate-mirrors archarm` | Arch Linux ARM | |
| `rate-mirrors arcolinux` | ArcoLinux | |
| `rate-mirrors artix` | Artix Linux | |
| `rate-mirrors blackarch` | BlackArch Linux | |
| `rate-mirrors cachyos` | CachyOS | Reads `code=XX` country metadata |
| `rate-mirrors chaotic-aur` | Chaotic-AUR | |
| `rate-mirrors archlinuxcn` | Arch Linux CN | |
| `rate-mirrors endeavouros` | EndeavourOS | Skips outdated mirrors |
| `rate-mirrors manjaro` | Manjaro | Skips outdated mirrors |
| `rate-mirrors rebornos` | RebornOS | |

### Other

| Command | Distribution |
|---------|-------------|
| `rate-mirrors openbsd` | OpenBSD |
| `rate-mirrors stdin` | Custom mirrors (see [below](#custom-mirrors-via-stdin)) |

## Where mirror lists come from

Before it can rank anything, `rate-mirrors` needs a list of candidate mirrors. It
looks for one in this order and uses the first that works:

1. **What you passed** — `--mirror-source` / `--mirror-list-file`, or the matching
   environment variable. If you name a source explicitly it is used exactly as
   given, with no fallback: naming one source and silently ranking another would
   be worse than failing.
2. **A file you control** — `/etc/rate-mirrors/sources/<distro>-mirrorlist.txt`.
   Drop a list here and that is the only set of mirrors the machine will ever
   consider. Nothing writes to this directory but you.
3. **Your distribution's packaged list** — for CachyOS, `/etc/pacman.d/cachyos-mirrorlist`,
   which arrives signed as part of a package and needs no network round trip.
4. **The list your distribution publishes** — fetched over HTTPS.

Every run prints which one it used:

```
# MIRROR SOURCE: skipping /etc/rate-mirrors/sources/cachyos-mirrorlist.txt (not present)
# MIRROR SOURCE: /etc/pacman.d/cachyos-mirrorlist (local)
```

A missing local file is normal and not an error — it just means the next source in
the chain is used.

### Pinning a mirror source

To decide once which mirrors a machine will use and stop consulting the network:

```bash
sudo mkdir -p /etc/rate-mirrors/sources
sudo cp my-vetted-mirrors.txt /etc/rate-mirrors/sources/cachyos-mirrorlist.txt

# Ranks only the mirrors in that file, and fails rather than fetching a list
rate-mirrors --no-remote-sources cachyos
```

`--mirror-source-sha256=<hex>` additionally refuses to proceed unless the source
content hashes to exactly what you expect.

> **Note on step 3.** The CachyOS wrapper writes ranked output back over
> `/etc/pacman.d/cachyos-mirrorlist`. Reading that back would re-rank only the
> survivors of the last run, shrinking the pool a little further every time. So the
> packaged list is only used while it still carries the `code=` country metadata
> that the shipped file has and our output does not; once rewritten, it is skipped.

## Mirror verification

Being fast says nothing about whether a mirror is serving the same packages as
everyone else. After ranking, and before writing anything out, each mirror is
cross-checked against the others.

Every mirror's repository database is fingerprinted:

- the detached signature next to it (`<db>.sig`), hashed — used where the repository
  publishes one, as CachyOS does
- otherwise the database's size — Arch [deliberately does not sign its
  databases](https://wiki.archlinux.org/title/Pacman/Package_signing), so for those
  repositories the size of a given database generation is the corroborating signal

Mirrors are then grouped by fingerprint, and **a mirror serving something no other
mirror corroborates is dropped**:

```
# ==== VERIFYING MIRRORS ====
#     https://us.cachyos.org/repo/ db.sig c270e9a66d16 - 12 agreeing, synced 8h ago, DNSSEC
#     https://mirror.hjk.gg/cachyos/repo/ db.sig c270e9a66d16 - 12 agreeing, synced 8h ago
#     CONSENSUS: 12/12 mirrors serve the same database
```

Corroboration is used rather than comparison against one trusted reference on
purpose: a reference host is one DNS answer away from being the attacker, whereas
agreeing with a dozen independently operated mirrors is not something a hijacked
resolver or a single bad operator can manufacture. Mirrors that are simply behind
still pass — a lagging database generation that two or more mirrors share is
corroborated, so honest mirrors are not punished for sync timing.

**Verification never returns an empty mirrorlist.** If every mirror fails, the
ranking is returned unverified with a warning, because an unattended installer
writing an empty list would leave a machine with no repositories at all.

### DNSSEC

Each mirror's hostname is checked against a validating resolver over HTTPS, out of
band, since a subverted local resolver cannot be asked to vouch for itself. Mirrors
in DNSSEC-signed zones are preferred over unsigned ones **of comparable speed**
(within 10%), so a meaningfully faster mirror is never demoted for being unsigned.

Signed zones are still a minority among distro mirrors, which is why this is a
preference rather than a requirement. `--require-dnssec` makes it a requirement and
will discard most of the pool; `--no-dnssec-check` skips the lookup entirely.

| Option | Description |
|--------|-------------|
| `--no-verify-mirrors` | Skip verification completely |
| `--max-mirror-age=HOURS` | Drop mirrors whose database is older than this |
| `--require-dnssec` | Drop mirrors not in a DNSSEC-signed zone |
| `--no-dnssec-check` | Skip the DNSSEC lookup |
| `--doh-resolver=URL` | Resolver to use (default: Cloudflare) |
| `--verify-timeout=MS` | Per-request timeout (default: 10000) |

## Choosing which mirrors you will accept

`--exclude-countries` takes comma-separated ISO country codes and applies to every
target:

```bash
rate-mirrors --exclude-countries=RU,CN cachyos
```

Every option has a matching environment variable, which is how to configure a
wrapper script or a graphical installer that calls `rate-mirrors` without letting
you pass flags:

```bash
export RATE_MIRRORS_EXCLUDE_COUNTRIES=RU,CN
sudo -E cachyos-rate-mirrors
```

Mirrors with no country metadata are kept by default; exclude them with the `ZZ`
pseudo-code. To go further and decide the entire candidate set yourself, pin a
mirror source as described [above](#pinning-a-mirror-source).

## Options

Run `rate-mirrors --help` for base options and `rate-mirrors <subcommand> --help`
for per-distribution ones.

| Option | Description | Default |
|--------|-------------|---------|
| `--save=FILE` | Save output to a file instead of stdout | - |
| `--concurrency=N` | Simultaneous speed tests | 16 |
| `--max-jumps=N` | Maximum country hops | 7 |
| `--entry-country=CC` | Starting country code | US |
| `--exclude-countries=CC,CC` | Exclude countries | - |
| `--protocol=PROTO` | Test only http or https | - |
| `--max-mirrors-to-output=N` | Maximum mirrors to output | - |
| `--disable-comments` | Do not print comments | false |
| `--disable-untested-fallback` | Fail instead of returning untested mirrors | false |
| `--allow-root` | Allow running as root | false |
| `--no-remote-sources` | Use only local mirror sources | false |
| `--mirror-source-sha256=HEX64` | Require the source to hash to this | - |

The tool does not need root. `--allow-root` exists for installers and wrappers that
already run as root.

## Algorithm

1. Fetch the candidate mirrors (see [above](#where-mirror-lists-come-from))
2. Filter by protocol, country, and distribution-specific freshness data
3. Starting from the entry country, find neighbours by major internet hubs (first
   two jumps) and geographic proximity (every jump)
4. Speed-test mirrors per country, tracking fastest and lowest-latency
5. Jump to the countries of the best mirrors and repeat
6. Re-test the top mirrors sequentially for a final ranking
7. Cross-check the survivors and drop uncorroborated ones

**Data attribution:** submarine cable and internet exchange data from
[TeleGeography](https://www2.telegeography.com).

## Examples

### Everyday use

```bash
alias ua-drop-caches='sudo paccache -rk3; yay -Sc --aur --noconfirm'
alias ua-update-all='export TMPFILE="$(mktemp)"; \
    sudo true; \
    rate-mirrors --save=$TMPFILE arch --max-delay=21600 \
      && sudo mv /etc/pacman.d/mirrorlist /etc/pacman.d/mirrorlist-backup \
      && sudo mv $TMPFILE /etc/pacman.d/mirrorlist \
      && ua-drop-caches \
      && yay -Syyu --noconfirm'
```

`sudo true` prompts for the password up front, `paccache` comes from
`pacman-contrib`, and `yay` is an AUR helper. Add to `~/.bashrc` and run
`ua-update-all`.

### Output

```
# STARTED AT: 2026-07-25 02:17:04 -04:00
# VERSION: 0.30.0
# ARGS: rate-mirrors --exclude-countries=RU cachyos
# MIRROR SOURCE: /etc/pacman.d/cachyos-mirrorlist (local)
# COUNTRY FILTER: 29 -> 25 mirrors
# JUMP #1
# EXPLORING US
#     + NEIGHBOR CA (by HubsFirst)
# [US] SpeedTestResult { speed: 54.5 MB/s; elapsed: 107ms; connection_time: 49ms }
# ...
# ==== VERIFYING MIRRORS ====
#     https://us.cachyos.org/repo/ db.sig c270e9a66d16 - 12 agreeing, synced 8h ago, DNSSEC
#     CONSENSUS: 12/12 mirrors serve the same database
# ==== RESULTS (top re-tested) ====
#   1. [US] SpeedTestResult { speed: 54.5 MB/s; ... } -> https://us.cachyos.org/repo/
Server = https://us.cachyos.org/repo/$arch/$repo
```

### Custom mirrors via stdin

For unsupported distributions or a hand-built list:

```bash
# Input: URL | COUNTRY<tab>URL | URL<tab>COUNTRY
cat mirrors.txt | rate-mirrors --concurrency=40 stdin \
    --path-to-test="extra/os/x86_64/extra.files" \
    --path-to-return='$repo/os/$arch' \
    --comment-prefix="# " \
    --output-prefix="Server = "
```

```
https://mirror-a.example.org/repo/
US	https://mirror-b.example.org/repo/
https://mirror-c.example.org/repo/	DE
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Error (network failure, invalid arguments, no usable mirror source, ...) |

## Credits and License

Originally written by **Nikita Almakov** as
[westandskif/rate-mirrors](https://github.com/westandskif/rate-mirrors), and
previously known as *Rate Arch Mirrors*. This repository is a fork maintained by
[8b-is](https://github.com/8b-is); the original work and the design of the
country-hopping ranking algorithm are his.

Licensed under
[Creative Commons Attribution-NonCommercial-ShareAlike 3.0 Unported (CC BY-NC-SA 3.0)](https://creativecommons.org/licenses/by-nc-sa/3.0/),
the same licence as the original, as ShareAlike requires.
