#!/usr/bin/env bash
#
# Fetch the prebuilt pdfium native library that zyndeck-ingester links against
# at runtime, into vendor/pdfium/. The library is platform-specific and not
# committed; run this once per machine (and in CI / Docker builds).
#
# Override the target with PDFIUM_PLATFORM (e.g. linux-x64, linux-arm64,
# mac-x64, mac-arm64) and the release with PDFIUM_RELEASE.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${REPO_ROOT}/vendor/pdfium"
RELEASE="${PDFIUM_RELEASE:-latest}"

# Guess the platform slug used by bblanchon/pdfium-binaries from the host.
if [[ -z "${PDFIUM_PLATFORM:-}" ]]; then
  case "$(uname -s)" in
    Darwin) os="mac" ;;
    Linux) os="linux" ;;
    *) echo "unsupported OS $(uname -s); set PDFIUM_PLATFORM" >&2; exit 1 ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch="x64" ;;
    arm64 | aarch64) arch="arm64" ;;
    *) echo "unsupported arch $(uname -m); set PDFIUM_PLATFORM" >&2; exit 1 ;;
  esac
  PDFIUM_PLATFORM="${os}-${arch}"
fi

if [[ "${RELEASE}" == "latest" ]]; then
  base="https://github.com/bblanchon/pdfium-binaries/releases/latest/download"
else
  base="https://github.com/bblanchon/pdfium-binaries/releases/download/${RELEASE}"
fi
url="${base}/pdfium-${PDFIUM_PLATFORM}.tgz"

echo "Fetching pdfium (${PDFIUM_PLATFORM}, ${RELEASE}) -> ${DEST}"
mkdir -p "${DEST}"
tmp="$(mktemp)"
trap 'rm -f "${tmp}"' EXIT
curl -fsSL -o "${tmp}" "${url}"
tar -xzf "${tmp}" -C "${DEST}"
echo "Done. Library at ${DEST}/lib"
