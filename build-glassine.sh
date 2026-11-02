#!/bin/sh
# Build Rnote Ink as a Flatpak (build-aux/org.glassine.RnoteInk.yaml) in a container:
#   build-glassine.sh [aarch64|x86_64] [OUT]     -> OUT/rnote-ink-ARCH.flatpak (a bundle)
# The SDK, its rust extension and flatpak-builder's cache live in OUT/fp-ARCH
# (several GB; reused; default OUT = ./out, keep it outside git). aarch64 runs under qemu (binfmt): about an hour.
# Install on the device: flatpak install --user --bundle rnote-ink-aarch64.flatpak
set -e
A=${1:-aarch64}
D=$(cd "$(dirname "$0")" && pwd)
OUT=$(mkdir -p "${2:-$D/out}" && cd "${2:-$D/out}" && pwd)
case $A in aarch64|x86_64) ;; *) echo "arch: aarch64|x86_64"; exit 2 ;; esac
mkdir -p "$OUT/fp-$A"
# a native (x86_64) container: bwrap needs real namespaces (none under qemu-user);
# only the build commands of an aarch64 build run under qemu (binfmt, F flag)
docker run --rm --privileged --platform linux/amd64 -e HOST_UID="$(id -u)" -v "$D":/m:ro -v "$OUT":/out alpine:3.24 sh -ec '
apk add -q flatpak flatpak-builder git patch >/dev/null
export FLATPAK_USER_DIR=/out/fp-'$A'/flatpak
flatpak --user remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
W=/out/fp-'$A'/w; mkdir -p $W; cd $W   # (the build dir: on the state dir filesystem)
flatpak-builder --arch='$A' --user --install-deps-from=flathub --force-clean --disable-rofiles-fuse --state-dir=/out/fp-'$A'/state --repo=/out/fp-'$A'/repo build /m/build-aux/org.glassine.RnoteInk.yaml >/out/build-'$A'.log 2>&1 || { grep -E "^error|error\[" -A12 /out/build-'$A'.log | head -80; tail -20 /out/build-'$A'.log; exit 1; }
flatpak build-bundle --arch='$A' --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo /out/fp-'$A'/repo /out/rnote-ink-'$A'.flatpak org.glassine.RnoteInk
chown -R $HOST_UID /out/rnote-ink-'$A'.flatpak /out/build-'$A'.log
sha256sum /out/rnote-ink-'$A'.flatpak
'
