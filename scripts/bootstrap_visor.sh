#!/bin/sh
# bootstrap_visor.sh — Descargar y preparar el motor en el VISOR, sin USB.
#
# Lo ejecuta la app del visor (que puede lanzar `sh`) cuando ve que falta el
# motor.  Usa solo herramientas que Android trae en toybox: wget, tar, gzip,
# zstd, mkdir, mv.
#
# Variables de entorno:
#   XFV_URL        base del servidor HTTP (por defecto http://10.0.2.2:8099
#                  hmm: se pasa desde la app; ver el launcher)
#   XFV_DESTINO    donde se deja (por defecto el directorio de datos de la app)
#
# Por que el motor tiene que vivir aqui: es la unica forma de que el motor
# (un binario glibc) pueda tocar la GPU del visor sin root, y es la regla que
# impone el propio motor al arrancar (su read-data/write-data).

set -u

URL="${XFV_URL:-http://172.20.5.110:8099}"
DEST="${XFV_DESTINO:-/data/data/com.xfactorvr.tabletop/files/factorio}"
TRABAJO="$DEST/.descarga"

log() { echo "[bootstrap] $*" ; }

wget_de() {
    # wget de toybox: -O destino.  Si falla, se avisa con codigo de salida.
    wget -O "$2" "$1" || return 1
}

log "servidor: $URL"
log "destino: $DEST"

mkdir -p "$TRABAJO" || { log "no se pudo crear $TRABAJO"; exit 1; }

# --- 1. La APK no hace falta aqui (ya esta instalada) ----------------------

# --- 2. El runtime de glibc ------------------------------------------------
if [ ! -f "$DEST/glibc/lib/ld-linux-aarch64.so.1" ]; then
    log "bajando el runtime de glibc..."
    if wget_de "$URL/glibc.tar.gz" "$TRABAJO/glibc.tar.gz"; then
        mkdir -p "$DEST/glibc" && tar xf "$TRABAJO/glibc.tar.gz" -C "$DEST/glibc" 2>/dev/null \
            || tar xzf "$TRABAJO/glibc.tar.gz" -C "$DEST/glibc" 2>/dev/null \
            || log "AVISO: no se pudo descomprimir el runtime de glibc"
    else
        log "ERROR: no se pudo bajar el runtime de glibc"
        exit 1
    fi
else
    log "el runtime de glibc ya esta"
fi

# --- 3. El payload de Mesa -------------------------------------------------
MESA_DIR="$DEST/mesa"
if [ ! -f "$MESA_DIR/usr/lib/aarch64-linux-gnu/libEGL_mesa.so.0" ]; then
    log "bajando el payload de Mesa..."
    if wget_de "$URL/mesa.tar.gz" "$TRABAJO/mesa.tar.gz"; then
        mkdir -p "$MESA_DIR" && tar xf "$TRABAJO/mesa.tar.gz" -C "$MESA_DIR" 2>/dev/null \
            || log "AVISO: no se pudo descomprimir el payload de Mesa"
    else
        log "ERROR: no se pudo bajar el payload de Mesa"
        exit 1
    fi
else
    log "el payload de Mesa ya esta"
fi

# --- 4. El motor grafico ---------------------------------------------------
if [ ! -f "$DEST/factorio/bin/arm64/factorio" ]; then
    log "bajando el motor grafico (son 1,5 GB, patience)..."
    if wget_de "$URL/factorio_cliente.tar" "$TRABAJO/factorio_cliente.tar"; then
        mkdir -p "$DEST/factorio" && tar xf "$TRABAJO/factorio_cliente.tar" -C "$DEST/factorio" 2>/dev/null \
            || log "AVISO: no se pudo descomprimir el motor"
    else
        log "ERROR: no se pudo bajar el motor"
        exit 1
    fi
else
    log "el motor grafico ya esta"
fi

# --- 5. El inyector y el mod ----------------------------------------------
if [ ! -f "$DEST/libxfactorvr_injector.so" ]; then
    log "bajando el inyector..."
    wget_de "$URL/libxfactorvr_injector.so" "$DEST/libxfactorvr_injector.so" || log "AVISO: sin inyector"
fi
if [ ! -f "$DEST/mods/xfactorvr_bridge/control.lua" ]; then
    log "bajando el mod puente..."
    mkdir -p "$DEST/mods/xfactorvr_bridge"
    wget_de "$URL/mod/control.lua" "$DEST/mods/xfactorvr_bridge/control.lua" || log "AVISO: sin mod"
    wget_de "$URL/mod/info.json" "$DEST/mods/xfactorvr_bridge/info.json" || true
fi

# --- 6. Listo --------------------------------------------------------------
log "listo. El motor esta en $DEST/factorio"
log "  glibc:  $DEST/glibc/lib/ld-linux-aarch64.so.1"
log "  mesa:  $MESA_DIR/usr/lib/aarch64-linux-gnu"
exit 0
