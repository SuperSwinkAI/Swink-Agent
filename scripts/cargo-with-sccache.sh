#!/usr/bin/env bash
set -euo pipefail

if [ -z "${SCCACHE_DISABLE:-}" ] && command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
else
  export RUSTC_WRAPPER=
fi

exec cargo "$@"
