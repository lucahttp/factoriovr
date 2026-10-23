#!/usr/bin/env bash
set -uo pipefail

# test_puente_entrada.sh — Compila y corre el banco de escritorio del cliente
# XFCE (android/src/puente_entrada.{h,cpp} + android/tests/test_puente_entrada.cpp).
#
# No hace falta la NDK ni el dispositivo: el modulo solo usa sockets POSIX.
#
# Codigos de salida:
#   0  todo correcto (compila sin avisos y el banco pasa)
#   1  fallo real (no compila, hay avisos, o el banco falla)
#
# Uso: bash scripts/test_puente_entrada.sh

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SRC="${PROJECT_ROOT}/android/src/puente_entrada.cpp"
HDR="${PROJECT_ROOT}/android/src/puente_entrada.h"
TEST="${PROJECT_ROOT}/android/tests/test_puente_entrada.cpp"

for f in "$SRC" "$HDR" "$TEST"; do
    if [[ ! -f "$f" ]]; then
        echo "FALLO: no existe $f"
        exit 1
    fi
done

# Compilador: el que haya en el Mac (clang++ de Apple o g++).
CXX="${CXX:-}"
if [[ -z "$CXX" ]]; then
    if command -v clang++ >/dev/null 2>&1; then
        CXX="clang++"
    elif command -v g++ >/dev/null 2>&1; then
        CXX="g++"
    else
        echo "FALLO: no hay clang++ ni g++ en el PATH"
        exit 1
    fi
fi
echo "Compilador: $CXX ($($CXX --version | head -1))"

# Directorio temporal de trabajo (se limpia al final con python, sin rm).
TMPDIR_TEST="$(mktemp -d "${TMPDIR:-/tmp}/xfv-puente-XXXXXX")"
BIN="${TMPDIR_TEST}/test_puente_entrada"
LOG="${TMPDIR_TEST}/compilacion.log"

echo "Compilando el banco..."
"$CXX" -std=c++17 -Wall -Wextra -O1 -o "$BIN" "$TEST" "$SRC" >"$LOG" 2>&1
RC=$?

if [[ $RC -ne 0 || ! -x "$BIN" ]]; then
    echo "FALLO: la compilacion fallo"
    cat "$LOG"
    python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1], ignore_errors=True)" "$TMPDIR_TEST"
    exit 1
fi

if [[ -s "$LOG" ]]; then
    echo "FALLO: la compilacion dio avisos (se exigen -Wall -Wextra limpios)"
    cat "$LOG"
    python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1], ignore_errors=True)" "$TMPDIR_TEST"
    exit 1
fi
echo "Compilacion limpia (sin avisos)."
echo ""

echo "Corriendo el banco..."
"$BIN"
RC=$?
echo ""

python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1], ignore_errors=True)" "$TMPDIR_TEST"

if [[ $RC -eq 0 ]]; then
    echo "RESULTADO: OK (0)"
    exit 0
fi
echo "RESULTADO: FALLO ($RC)"
exit 1
