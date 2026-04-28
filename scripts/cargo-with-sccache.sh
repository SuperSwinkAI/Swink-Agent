#!/usr/bin/env bash
set -euo pipefail

if command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
else
  export RUSTC_WRAPPER=
fi

exec cargo "$@"
