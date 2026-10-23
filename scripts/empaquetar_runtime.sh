#!/usr/bin/env bash
# empaquetar_runtime.sh — Empaqueta el MODULO DE RUNTIME en un solo fichero.
#
# El runtime es lo que permite ejecutar el binario Linux ARM64 de Factorio
# dentro de Android SIN Termux (ver runtime/README.md):
#
#   glibc/   el cargador y las libs de glibc
#   mesa/    Mesa con Zink + lavapipe + libGLESv2 (el driver "offscreen" de
#            SDL2, que es el que funciona en Android sin servidor X)
#   x11/     un servidor X virtual (Xvfb), por si se quiere el driver "x11"
#
# El motor (324 MB) NO entra aqui: va aparte, en el despliegue.
#
# Salida: build/runtime.tar.gz
#
# Uso: bash scripts/empaquetar_runtime.sh

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/build"
STAGE="$BUILD_DIR/runtime-stage"
GLIBC_DIR="${GLIBC_DIR:-$BUILD_DIR/glibc}"
MESA_DIR="${MESA_DIR:-$BUILD_DIR/mesa}"
X11_DIR="${X11_DIR:-$PROJECT_ROOT/runtime/x11}"
SALIDA="$BUILD_DIR/runtime.tar.gz"

log() { printf '%s\n' "$*"; }

[ -d "$GLIBC_DIR" ] || { log "ERROR: no esta el runtime de glibc en $GLIBC_DIR"; log "       (se genera con scripts/prepare_glibc.sh)"; exit 1; }
[ -d "$MESA_DIR" ]  || { log "ERROR: no esta el payload de Mesa en $MESA_DIR"; log "       (se genera con scripts/prepare_mesa.py)"; exit 1; }

log "== Empaquetando el modulo de runtime =="
rm -rf "$STAGE" 2>/dev/null || true
mkdir -p "$STAGE/glibc" "$STAGE/mesa"

# glibc: el cargador y las libs (sin la docena de ficheros que no hacen falta).
cp -a "$GLIBC_DIR/." "$STAGE/glibc/"
log "  glibc: $(du -sk "$STAGE/glibc" | cut -f1) KB"

# Mesa: el payload tal cual (con libGLESv2 y el driver Zink).
cp -a "$MESA_DIR/." "$STAGE/mesa/"
log "  mesa:  $(du -sk "$STAGE/mesa" | cut -f1) KB"

# X11: el servidor X virtual.
if [ -d "$X11_DIR" ] && ls "$X11_DIR"/*.tar.gz >/dev/null 2>&1; then
    mkdir -p "$STAGE/x11"
    tar xzf "$(ls "$X11_DIR"/*.tar.gz | head -1)" -C "$STAGE/x11" --strip-components=1
    log "  x11:   $(du -sk "$STAGE/x11" | cut -f1) KB"
else
    log "  AVISO: no hay modulo X11 en $X11_DIR"
fi

# El manifiesto: que hay dentro y de donde sale cada cosa.
python3 - "$STAGE" "$SALIDA" <<'PY'
import json, os, sys
stage, salida = sys.argv[1], sys.argv[2]
manifiesto = {
    "modulo": "xfactorvr-runtime",
    "version": 1,
    "para": "ejecutar el binario Linux ARM64 de Factorio dentro de Android",
    "sin_termux": True,
    "piezas": {
        "glibc": {"que": "cargador y libs de glibc 2.44", "origen": "paquete glibc de Termux"},
        "mesa": {"que": "Mesa con Zink + lavapipe + libGLESv2", "origen": "scripts/prepare_mesa.py (paquetes .deb de Debian)"},
        "x11": {"que": "servidor X virtual (Xvfb)", "origen": "contenedor de pruebas"},
    },
    "despliegue": {
        "glibc": "/data/local/tmp/glibc",
        "mesa": "/data/local/tmp/mesa",
        "x11": "/data/local/tmp/x11mod",
    },
    "notas": [
        "El motor no necesita X11: SDL2 usa EGL/GLES con el driver offscreen.",
        "El ICD de Adreno es bionic y no lo puede cargar un proceso glibc: se usa lavapipe.",
        "El motor headless ya arranca con este runtime (RCON en 127.0.0.1:27015).",
    ],
}
with open(os.path.join(stage, "manifest.json"), "w", encoding="utf-8") as f:
    json.dump(manifiesto, f, indent=2, ensure_ascii=False)
print("  manifiesto escrito")
PY

tar czf "$SALIDA" -C "$STAGE" .
log "== Listo: $SALIDA ($(du -k "$SALIDA" | cut -f1) KB) =="
log "   desplegar con: bash scripts/deploy_pico.sh --runtime"
