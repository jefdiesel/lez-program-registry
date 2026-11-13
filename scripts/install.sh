#!/bin/sh
# Installs a released `lezreg` (the LEZ Program Registry command line) for this user, no sudo.
#
#   curl --proto '=https' --tlsv1.2 -fsSL https://github.com/jefdiesel/lez-program-registry/releases/latest/download/install.sh | sh
#   curl ... | sh -s -- --with-r0vm --with-storage     # options go after `-s --`
#   sh install.sh --dry-run                            # print the plan only
#
# What it does: picks the build for this OS and CPU, downloads it and the release's SHA256SUMS
# from GitHub over HTTPS, refuses unless the SHA-256 matches, and copies `lezreg` into
# ${LEZREG_HOME:-$HOME/.lezreg}/bin. It edits no shell startup file: it prints the PATH line for
# you to add. With --with-r0vm / --with-storage it also fetches RISC Zero's `r0vm` 3.0.5 and the
# Logos Storage v0.5.3 node from their own GitHub releases, each checked against a SHA-256
# pinned in this script.
#
# What the checks prove, and what they don't: the SHA-256 check catches a damaged or truncated
# download. It is not a signature: SHA256SUMS comes from the same GitHub release as the
# tarball, so your trust rests on GitHub's TLS and on whoever can write to this repository.
# To check where a file was built, use the attestation (`gh attestation verify`, see the README).
#
# Privacy: the script sends nothing about you or this machine. Like any download, GitHub (and
# the hosts it redirects to) sees your IP address and your curl or wget user agent.
#
# Options:
#   --version X     install release X (e.g. 0.1.0) instead of the latest. Env: LEZREG_VERSION.
#   --with-r0vm     also install r0vm 3.0.5, which every lezreg write needs (macOS arm64 and
#                   Linux x86_64 only: RISC Zero publishes no other prebuilt r0vm 3.0.5).
#   --with-storage  also install the Logos Storage v0.5.3 node as `logos-storage`, which
#                   `lezreg storage up` starts to upload your IDL (Linux: glibc 2.38 or newer).
#   --allow-root    install even when run as root (refused by default).
#   --dry-run       print what would be downloaded and installed, and stop.
#   -h, --help      this text.
# Environment: LEZREG_HOME (default ~/.lezreg), LEZREG_VERSION, LEZREG_BASE_URL (an https://
# mirror, or file:///dir holding the release files, for tests).

set -eu

REPO=jefdiesel/lez-program-registry
# r0vm 3.0.5: RISC Zero's release assets and their SHA-256 (GitHub's recorded digests).
R0VM_URL=https://github.com/risc0/risc0/releases/download/v3.0.5
R0VM_MACOS_ARM64=0f676055ad88d7ede06beaec3b7f850413e091db08bb7cf9969a78d9ad168562
R0VM_LINUX_X86_64=936ef988b78f20e3bd9f80e375f3adc934b13addc6ae2680f2e5fc0bcc966158
# Logos Storage v0.5.3: logos-storage/logos-storage-nim release assets and their SHA-256.
STORAGE_URL=https://github.com/logos-storage/logos-storage-nim/releases/download/v0.5.3
STORAGE_MACOS_ARM64=7f32f62a14e555399ee003f11d1e29bba9b6ff842d973f38b5b280f018c2f6ab
STORAGE_LINUX_X86_64=1086fae4f283cab882f133abe4d5a88cfae8d61edba9f0094c7f75b368ec9d56
STORAGE_LINUX_AARCH64=ecbc32536565096b75b2330dab4ff47a993714a1f3ce345099396968bc69796b

installed=

say() { printf '%s\n' "$*"; }
die() {
  printf 'lezreg install: %s\n' "$*" >&2
  if [ -n "$installed" ]; then
    printf 'lezreg install: already installed before this error:%s\n' "$installed" >&2
  fi
  exit 1
}

usage() {
  cat <<EOF
Installs a released lezreg into \${LEZREG_HOME:-\$HOME/.lezreg}/bin (no sudo).

  curl --proto '=https' --tlsv1.2 -fsSL https://github.com/$REPO/releases/latest/download/install.sh | sh -s -- [options]

  --version X     install release X (e.g. 0.1.0) instead of the latest (env LEZREG_VERSION)
  --with-r0vm     also install RISC Zero's r0vm 3.0.5, which every write needs
                  (macOS arm64 and Linux x86_64: the only prebuilt r0vm 3.0.5 builds)
  --with-storage  also install the Logos Storage v0.5.3 node as logos-storage
  --allow-root    install even when run as root (refused by default)
  --dry-run       print what would be downloaded and installed, and stop
  -h, --help      this text

Environment: LEZREG_HOME, LEZREG_VERSION, LEZREG_BASE_URL (https:// mirror or file:///dir).
The SHA-256 check catches a damaged download; provenance is checked with gh attestation verify.
More: https://github.com/$REPO#install
EOF
}

# The SHA-256 of a file, by whichever tool this system has; empty if none.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 -r "$1" | cut -d' ' -f1
  fi
}

# Downloads over HTTPS only (TLS 1.2 or newer); file:// is a local copy, for tests and mirrors.
fetch() { # url dest
  case $1 in
    file://*) cp "${1#file://}" "$2"; return ;;
    https://*) ;;
    *) die "refusing a non-https URL: $1" ;;
  esac
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q --https-only --secure-protocol=TLSv1_2 -O "$2" "$1"
  else
    die "needs curl or wget to download"
  fi
}

check_sha() { # file want what
  got=$(sha256_of "$1")
  [ -n "$got" ] || die "no SHA-256 tool (sha256sum, shasum or openssl) found: refusing to install an unchecked $3"
  [ "$got" = "$2" ] || die "SHA-256 mismatch for $3: expected $2, got $got. $3 was not installed."
}

# Copies $1 to $2/$3 with mode $4 without ever writing through a name someone else could have
# planted: a fresh mktemp file in the same directory, then a rename over the final name (a
# rename replaces a symlink there instead of following it).
put() { # src dir name mode
  t=$(mktemp "$2/.lezreg.XXXXXX") || die "cannot create a temporary file in $2"
  if cp "$1" "$t" && chmod "$4" "$t" && mv -f "$t" "$2/$3"; then
    return 0
  fi
  rm -f "$t"
  die "cannot install $2/$3"
}

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case $os in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) os=unsupported ;;
  esac
  case $arch in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) if [ "$os" = linux ]; then arch=aarch64; else arch=arm64; fi ;;
    *) arch=unsupported ;;
  esac
  # Apple silicon under Rosetta reports x86_64; the native build is the right one.
  if [ "$os" = macos ] && [ "$arch" = x86_64 ] &&
    [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
    arch=arm64
  fi
  target=$os-$arch
}

unsupported() {
  die "no prebuilt lezreg for $(uname -s) $(uname -m). Build it from source instead:
  https://github.com/$REPO#build-from-source
(Windows: use WSL and run this installer inside it.)"
}

# This system's glibc version as "2.35"; empty if it reports none (musl, or no getconf).
glibc_version() {
  getconf GNU_LIBC_VERSION 2>/dev/null | sed -n 's/^glibc \([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p'
}
version_num() { # "2.35" -> 2035 (the input is digits.digits, from glibc_version)
  echo $((${1%%.*} * 1000 + ${1#*.}))
}

main() {
  version=${LEZREG_VERSION:-}
  with_r0vm=0
  with_storage=0
  allow_root=0
  dry=0
  while [ $# -gt 0 ]; do
    case $1 in
      --version) [ $# -ge 2 ] || die "--version needs a value"; version=$2; shift ;;
      --version=*) version=${1#--version=} ;;
      --with-r0vm) with_r0vm=1 ;;
      --with-storage) with_storage=1 ;;
      --allow-root) allow_root=1 ;;
      --dry-run) dry=1 ;;
      -h | --help) usage; exit 0 ;;
      *) die "unknown option: $1 (try --help)" ;;
    esac
    shift
  done

  # Every tool used below, checked first, so a missing one is named as such.
  for t in uname id sed grep head cut mktemp tar gzip cp mv chmod mkdir rm; do
    command -v "$t" >/dev/null 2>&1 || die "needs $t, which is not on PATH"
  done

  if [ "$(id -u)" = 0 ] && [ "$allow_root" != 1 ]; then
    die "refusing to run as root: lezreg installs per user, no sudo needed. Run it as your own
user, or pass --allow-root (e.g. in a container) to install into ${LEZREG_HOME:-\$HOME/.lezreg}."
  fi

  version=${version#v}
  case $version in
    "") ;;
    [0-9]*)
      case $version in
        *[!0-9A-Za-z.-]*) die "bad version '$version': only digits, letters, '.' and '-' (e.g. 0.1.0)" ;;
      esac
      ;;
    *) die "bad version '$version': it must start with a digit (e.g. 0.1.0)" ;;
  esac

  detect_target
  case $target in
    linux-x86_64 | linux-aarch64 | macos-arm64) ;;
    *) unsupported ;;
  esac

  glibc=
  if [ "$os" = linux ]; then
    glibc=$(glibc_version)
    [ -n "$glibc" ] || die "the Linux builds of lezreg need glibc (this system does not report one;
musl distributions such as Alpine are not supported). Use a glibc distribution or container,
or build from source: https://github.com/$REPO#build-from-source"
    [ "$(version_num "$glibc")" -ge 2034 ] || die "the Linux builds of lezreg need glibc 2.34 or
newer; this system has glibc $glibc. Build from source: https://github.com/$REPO#build-from-source"
  fi

  [ -n "${LEZREG_HOME:-}" ] || [ -n "${HOME:-}" ] || die "neither LEZREG_HOME nor HOME is set"
  home=${LEZREG_HOME:-$HOME/.lezreg}
  case $home in
    /*) ;;
    *) home=$(pwd)/$home ;;
  esac
  bin=$home/bin

  if [ -n "${LEZREG_BASE_URL:-}" ]; then
    base=${LEZREG_BASE_URL%/}
    case $base in
      https://* | file:///*) ;;
      *) die "LEZREG_BASE_URL must be https://... or file:///...: refusing '$base'" ;;
    esac
  elif [ -n "$version" ]; then
    base=https://github.com/$REPO/releases/download/v$version
  else
    base=https://github.com/$REPO/releases/latest/download
  fi

  r0_asset=
  r0_sha=
  case $target in
    macos-arm64) r0_asset=cargo-risczero-aarch64-apple-darwin.tgz; r0_sha=$R0VM_MACOS_ARM64 ;;
    linux-x86_64) r0_asset=cargo-risczero-x86_64-unknown-linux-gnu.tgz; r0_sha=$R0VM_LINUX_X86_64 ;;
  esac
  st_asset=
  st_sha=
  case $target in
    macos-arm64) st_asset=logos-storage-darwin-arm64-v0.5.3; st_sha=$STORAGE_MACOS_ARM64 ;;
    linux-x86_64) st_asset=logos-storage-linux-amd64-v0.5.3; st_sha=$STORAGE_LINUX_X86_64 ;;
    linux-aarch64) st_asset=logos-storage-linux-arm64-v0.5.3; st_sha=$STORAGE_LINUX_AARCH64 ;;
  esac
  st_why="Logos Storage v0.5.3 has no build for $target"
  # The official Linux builds of Logos Storage v0.5.3 need glibc 2.38 (Ubuntu 24.04, Debian 13,
  # Fedora 39 and newer); on older systems they do not start, so do not install them there.
  if [ "$os" = linux ] && [ "$(version_num "$glibc")" -lt 2038 ]; then
    st_asset=
    st_why="the Logos Storage v0.5.3 Linux build needs glibc 2.38 or newer, and this system has glibc $glibc"
  fi

  if [ "$dry" = 1 ]; then
    say "Dry run: nothing is downloaded or written."
    say "  platform:  $target${glibc:+ (glibc $glibc)}"
    say "  release:   ${version:-latest} from $base"
    say "  download:  $base/SHA256SUMS"
    say "  download:  $base/lezreg-${version:-<version>}-$target.tar.gz (checked against SHA256SUMS)"
    say "  install:   $bin/lezreg"
    if [ "$with_r0vm" = 1 ]; then
      if [ -n "$r0_asset" ]; then
        say "  download:  $R0VM_URL/$r0_asset (SHA-256 $r0_sha), install $bin/r0vm"
      else
        say "  r0vm:      no prebuilt r0vm 3.0.5 for $target; would print how to build it"
      fi
    fi
    if [ "$with_storage" = 1 ]; then
      if [ -n "$st_asset" ]; then
        say "  download:  $STORAGE_URL/$st_asset.zip (SHA-256 $st_sha), install $bin/logos-storage"
      else
        say "  storage:   skipped: $st_why"
      fi
    fi
    exit 0
  fi

  [ -n "$(sha256_of /dev/null)" ] || die "no SHA-256 tool (sha256sum, shasum or openssl) found: refusing to install unchecked files"

  tmp=$(mktemp -d 2>/dev/null || mktemp -d -t lezreg)
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 1' INT TERM HUP

  say "Downloading lezreg (${version:-latest}) for $target..."
  fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "cannot download $base/SHA256SUMS"
  # Only a well-formed line for this platform: 64 hex digits, two spaces, a plain file name.
  line=$(grep -E "^[0-9a-f]{64}  lezreg-[0-9A-Za-z._-]+-$target\.tar\.gz\$" "$tmp/SHA256SUMS" | head -n 1 || true)
  [ -n "$line" ] || die "SHA256SUMS from $base lists no lezreg build for $target"
  want=${line%% *}
  name=${line##* }
  [ -z "$version" ] || [ "$name" = "lezreg-$version-$target.tar.gz" ] ||
    die "SHA256SUMS lists $name, not version $version"
  fetch "$base/$name" "$tmp/$name" || die "cannot download $base/$name"
  check_sha "$tmp/$name" "$want" "$name"
  mkdir "$tmp/x"
  tar -xzf "$tmp/$name" -C "$tmp/x"
  dir=$tmp/x/${name%.tar.gz}
  [ -f "$dir/lezreg" ] || die "$name has no lezreg in it"

  mkdir -p "$bin" "$home/share/lezreg"
  put "$dir/lezreg" "$bin" lezreg 755
  installed="$installed $bin/lezreg"
  for f in "$dir"/LICENSE*; do
    [ -f "$f" ] && put "$f" "$home/share/lezreg" "${f##*/}" 644
  done
  say "Installed $bin/lezreg ($("$bin/lezreg" --version 2>/dev/null || echo "version unknown"))"

  if [ "$with_r0vm" = 1 ]; then
    if [ -z "$r0_asset" ]; then
      say ""
      say "--with-r0vm: RISC Zero publishes no prebuilt r0vm 3.0.5 for $target; skipped (see below)."
    else
      say "Downloading RISC Zero's r0vm 3.0.5 (about 70 MB) from github.com/risc0/risc0..."
      fetch "$R0VM_URL/$r0_asset" "$tmp/r0.tgz" || die "cannot download $R0VM_URL/$r0_asset"
      check_sha "$tmp/r0.tgz" "$r0_sha" "$r0_asset (r0vm)"
      mkdir "$tmp/r0"
      tar -xzf "$tmp/r0.tgz" -C "$tmp/r0" r0vm || die "$r0_asset has no r0vm in it"
      put "$tmp/r0/r0vm" "$bin" r0vm 755
      installed="$installed $bin/r0vm"
      say "Installed $bin/r0vm"
    fi
  fi

  if [ "$with_storage" = 1 ]; then
    if [ -z "$st_asset" ]; then
      say ""
      say "--with-storage: skipped: $st_why (see below)."
    elif ! command -v unzip >/dev/null 2>&1; then
      say ""
      say "--with-storage: needs unzip (the release is a .zip); skipped. Install unzip and rerun."
    else
      say "Downloading Logos Storage v0.5.3 from github.com/logos-storage/logos-storage-nim..."
      fetch "$STORAGE_URL/$st_asset.zip" "$tmp/st.zip" || die "cannot download $STORAGE_URL/$st_asset.zip"
      check_sha "$tmp/st.zip" "$st_sha" "$st_asset.zip (Logos Storage)"
      mkdir "$tmp/st"
      unzip -q "$tmp/st.zip" "$st_asset" -d "$tmp/st" || die "$st_asset.zip has no $st_asset in it"
      if "$tmp/st/$st_asset" --version >/dev/null 2>&1; then
        put "$tmp/st/$st_asset" "$bin" logos-storage 755
        installed="$installed $bin/logos-storage"
        say "Installed $bin/logos-storage"
      else
        say "--with-storage: the downloaded Logos Storage node does not run on this system; not installed (see below)."
      fi
    fi
  fi

  say ""
  case ":${PATH:-}:" in
    *":$bin:"*) say "$bin is already on your PATH." ;;
    *)
      say "Add lezreg to your PATH (this installer does not edit your shell files):"
      say "  export PATH=\"$bin:\$PATH\""
      say "Put that line in ~/.zshrc, ~/.bashrc or ~/.profile to keep it."
      ;;
  esac

  say ""
  say "Reads (lezreg show, list, search, idl get) work now. Two more things, for writes:"
  say ""
  if [ -x "$bin/r0vm" ]; then
    say "1. r0vm: installed in $bin."
  else
    say "1. r0vm 3.0.5, RISC Zero's executor. Every write (register, update, publish, adopt,"
    say "   deploy) is simulated with it before anything is sent; without it a write stops first."
    if [ -n "$r0_asset" ]; then
      say "   Rerun this installer with --with-r0vm, or use RISC Zero's own installer:"
      say "     curl -L https://risczero.com/install | bash && rzup install cargo-risczero 3.0.5"
    else
      say "   RISC Zero publishes no prebuilt r0vm 3.0.5 for $target. Build it with Rust:"
      say "     cargo install --locked risc0-r0vm --version 3.0.5 --root \"$home\""
    fi
    say "   lezreg finds it on PATH, or set RISC0_SERVER_PATH to the binary."
  fi
  say ""
  if [ -x "$bin/logos-storage" ]; then
    say "2. Logos Storage node: installed in $bin as logos-storage; \`lezreg storage up\` starts it."
  else
    say "2. A Logos Storage node, to upload your program's IDL when you register (an IDL is required)."
    say "   \`lezreg storage up\` starts one; it needs the Logos Storage v0.5.3 node as \`logos-storage\`"
    say "   on PATH (or --bin / LEZREG_STORAGE_BIN). Official builds:"
    say "     https://github.com/logos-storage/logos-storage-nim/releases/tag/v0.5.3"
    if [ -n "$st_asset" ]; then
      say "   or rerun this installer with --with-storage. Or point LEZREG_STORAGE_API at a node you run."
    else
      say "   Not here: $st_why. Point LEZREG_STORAGE_API at a node you run on another machine."
    fi
  fi
  say ""
  say "Then check this machine with: lezreg doctor"
}

main "$@"
