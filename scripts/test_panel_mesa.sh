#!/usr/bin/env bash
# test_panel_mesa.sh — Compila y corre el banco de escritorio del modulo del
# panel tumbado (android/src/panel_mesa.{h,cpp} + android/tests/test_panel_mesa.cpp).
#
# El modulo es matematicas puras: no necesita NDK, ni OpenXR, ni Android.
# Se compila con clang++ (o g++ si no hay clang) en el Mac y se ejecuta.
#
# Codigos de salida:
#   0  el banco pasa entero
#   1  el banco falla (o no compila)
#
# Uso: bash scripts/test_panel_mesa.sh

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_SRC="${PROJECT_ROOT}/android/tests/test_panel_mesa.cpp"
MOD_SRC="${PROJECT_ROOT}/android/src/panel_mesa.cpp"
BUILD_DIR="${PROJECT_ROOT}/build/pruebas"
BIN="${BUILD_DIR}/test_panel_mesa"

if ! command -v clang++ >/dev/null 2>&1; then
    if command -v g++ >/dev/null 2>&1; then
        CXX="g++"
    else
        echo "ERROR: no hay clang++ ni g++ en el PATH" >&2
        exit 1
    fi
else
    CXX="clang++"
fi

mkdir -p "${BUILD_DIR}"

echo "Compilando el banco del panel tumbado con ${CXX}..."
"${CXX}" -std=c++17 -O1 -g -Wall -Wextra -o "${BIN}" "${TEST_SRC}" "${MOD_SRC}"
if [ $? -ne 0 ]; then
    echo "ERROR: no ha compilado" >&2
    exit 1
fi

echo "Ejecutando el banco..."
"${BIN}"
CODIGO=$?

if [ ${CODIGO} -eq 0 ]; then
    echo "OK: el banco del panel tumbado pasa entero"
else
    echo "FALLO: el banco del panel tumbado ha fallado" >&2
fi
exit ${CODIGO}
