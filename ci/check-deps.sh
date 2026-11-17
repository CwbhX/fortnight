#!/usr/bin/env bash
# Fails if a built executable depends on anything beyond the platform baseline
# (PLAN.md section 3.1).
#
#   ci/check-deps.sh <binary> <macos|linux>
#
# macOS: `otool -L`. Allowed: libSystem and a few base system libraries in
#   /usr/lib, and anything under /System/Library/Frameworks (all ship with the OS).
# Linux: `readelf -d` NEEDED entries (works on foreign architectures, unlike
#   ldd). Allowed: the glibc family, libm, libdl, libpthread, librt, libgcc_s.
#   The GUI's X11, Wayland, GPU (Vulkan/EGL), xkbcommon and D-Bus-less portal
#   code all load at run time via dlopen (winit "x11"/"wayland-dlopen",
#   wayland-sys "dlopen", x11-dl, ash/libloading, khronos-egl dynamic), so none
#   of them may appear as NEEDED. If one does, that is the bug this script reports.
#   Set MAX_GLIBC=2.28 to also fail if the binary references newer glibc symbols.
#
# Windows has its own script: ci/check-deps.ps1.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <binary> <macos|linux>" >&2
  exit 2
fi
bin="$1"
platform="$2"

if [ ! -f "$bin" ]; then
  echo "check-deps: $bin does not exist" >&2
  exit 2
fi

bad=()

case "$platform" in
  macos)
    # First line is the file name; each following line is "<path> (compatibility ...)".
    deps=$(otool -L "$bin" | tail -n +2 | sed -E 's/^[[:space:]]+//; s/ \(compatibility.*$//')
    while IFS= read -r dep; do
      [ -z "$dep" ] && continue
      case "$dep" in
        /System/Library/Frameworks/*) ;;
        /usr/lib/libSystem.B.dylib) ;;
        /usr/lib/libobjc.A.dylib) ;;
        /usr/lib/libiconv.2.dylib) ;;
        /usr/lib/libc++.1.dylib) ;;
        /usr/lib/libc++abi.dylib) ;;
        /usr/lib/libresolv.9.dylib) ;;
        /usr/lib/libz.1.dylib) ;;
        *) bad+=("$dep") ;;
      esac
    done <<< "$deps"
    ;;
  linux)
    needed=$(readelf -d "$bin" | sed -n -E 's/.*\(NEEDED\).*\[(.*)\].*/\1/p')
    while IFS= read -r dep; do
      [ -z "$dep" ] && continue
      case "$dep" in
        libc.so.6 | libm.so.6 | libdl.so.2 | libpthread.so.0 | librt.so.1 | libutil.so.1 | libgcc_s.so.1) ;;
        ld-linux-x86-64.so.2 | ld-linux-aarch64.so.1 | ld-linux-armhf.so.3) ;;
        *) bad+=("$dep") ;;
      esac
    done <<< "$needed"

    if [ -n "${MAX_GLIBC:-}" ]; then
      # Highest GLIBC_x.y version the binary asks for.
      newest=$(readelf --dyn-syms -W "$bin" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1 || true)
      if [ -n "$newest" ]; then
        highest=$(printf '%s\n%s\n' "$MAX_GLIBC" "$newest" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
        if [ "$highest" != "$MAX_GLIBC" ]; then
          bad+=("glibc symbol version GLIBC_$newest (limit: GLIBC_$MAX_GLIBC)")
        fi
      fi
    fi
    ;;
  *)
    echo "check-deps: unknown platform '$platform' (expected macos or linux)" >&2
    exit 2
    ;;
esac

if [ "${#bad[@]}" -ne 0 ]; then
  echo "check-deps: FAIL: $bin has dependencies outside the $platform allow-list:" >&2
  for d in "${bad[@]}"; do echo "  - $d" >&2; done
  echo "The executable must be self-contained apart from the platform baseline (PLAN.md 3.1)." >&2
  exit 1
fi

echo "check-deps: OK: $bin ($platform)"
