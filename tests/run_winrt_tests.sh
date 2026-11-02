#!/bin/bash
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WINE="${WINE:-wine}"
WINEPREFIX="${WINEPREFIX:-$HOME/.wine}"
OUT_EXE="$SCRIPT_DIR/winrt_regression_tests.exe"

echo "Compiling WinRT regression tests..."
x86_64-w64-mingw32-gcc -O2 -o "$OUT_EXE" "$SCRIPT_DIR/winrt_regression_tests.c" -lole32 -lkernel32

echo "Running WinRT regression tests under $WINE (prefix: $WINEPREFIX)..."
WINEPREFIX="$WINEPREFIX" "$WINE" "$OUT_EXE"
