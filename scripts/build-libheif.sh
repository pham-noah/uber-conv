#!/usr/bin/env bash
# Builds static libheif + libde265 (MSVC x64, Release) so uberconv can decode HEIC
# on its own, without Windows' HEIF codec.
#
#   ./scripts/build-libheif.sh                 # -> ./libheif-install
#   PREFIX=/c/deps/libheif ./scripts/build-libheif.sh
#   UBERCONV_LIBHEIF_DIR="$PWD/libheif-install" cargo build --release
#
# Needs: git, CMake, Visual Studio 2022 (MSVC x64) with the Windows SDK, network.
# Verified with libde265 1.1.0 + libheif 1.19.8: decodes a 1280x854 HEVC HEIC to
# correct RGB. Expect ~22 MB of static libs; the resulting exe needs the MSVC C++
# runtime (MSVCP140.dll), which a /MT rebuild of the libs would remove (untested).
#
# If MSBuild reports "Windows SDK version ... was not found" on a machine whose SDK
# install is missing `sdkmanifest.xml`, set WindowsSDKProps to a props file that
# hardcodes WindowsSdkDir — env vars reach both CMake's probe and the build.
#
# The build is MSVC-specific. On Linux/macOS use your distro's/-brew libheif and
# point UBERCONV_LIBHEIF_DIR at that prefix instead.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$ROOT/libheif-install}"
WORK="${WORK:-$ROOT/libheif-build}"
CMAKE="${CMAKE:-cmake}"
GENERATOR="${GENERATOR:-Visual Studio 17 2022}"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 4)}"

mkdir -p "$WORK"
cd "$WORK"

[ -d libde265 ] || git clone --depth 1 --branch v1.1.0 https://github.com/strukturag/libde265.git
[ -d libheif ]  || git clone --depth 1 --branch v1.19.8 https://github.com/strukturag/libheif.git

# --- libde265 (decode only) -------------------------------------------------
"$CMAKE" -S libde265 -B build-de265 -G "$GENERATOR" -A x64 \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DBUILD_SHARED_LIBS=OFF -DENABLE_SDL=OFF -DENABLE_ENCODER=OFF \
  -DENABLE_SHERLOCK265=OFF -DENABLE_DECODER=ON -DENABLE_SIMD=ON
"$CMAKE" --build build-de265 --config Release --target install --parallel "$JOBS"

# --- libheif -----------------------------------------------------------------
# CMAKE_CXX_FLAGS=/DLIBDE265_STATIC_BUILD is mandatory: libde265 sets that define
# privately in its own tree and does not export it, so without it decoder_libde265
# references __imp_de265_* and the link fails with ~20 unresolved externals.
"$CMAKE" -S libheif -B build-heif -G "$GENERATOR" -A x64 \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_PREFIX_PATH="$PREFIX" \
  -DCMAKE_CXX_FLAGS="/DLIBDE265_STATIC_BUILD" \
  -DBUILD_SHARED_LIBS=OFF -DENABLE_PLUGIN_LOADING=OFF \
  -DWITH_LIBDE265=ON -DWITH_X265=OFF -DWITH_KVAZAAR=OFF -DWITH_UVG266=OFF \
  -DWITH_VVDEC=OFF -DWITH_VVENC=OFF -DWITH_OpenH264_DECODER=OFF -DWITH_DAV1D=OFF \
  -DWITH_AOM_DECODER=OFF -DWITH_AOM_ENCODER=OFF -DWITH_SvtEnc=OFF -DWITH_RAV1E=OFF \
  -DWITH_JPEG_DECODER=OFF -DWITH_JPEG_ENCODER=OFF -DWITH_OpenJPEG_ENCODER=OFF \
  -DWITH_OpenJPEG_DECODER=OFF -DWITH_FFMPEG_DECODER=OFF -DWITH_OPENJPH_ENCODER=OFF \
  -DWITH_LIBSHARPYUV=OFF -DWITH_UNCOMPRESSED_CODEC=ON \
  -DWITH_EXAMPLES=OFF -DWITH_GDK_PIXBUF=OFF
"$CMAKE" --build build-heif --config Release --target heif --parallel "$JOBS"
"$CMAKE" --install build-heif --config Release

echo
echo "Done. Build against it with:"
echo "  UBERCONV_LIBHEIF_DIR=\"$PREFIX\" cargo build --release"
