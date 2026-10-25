#!/bin/bash
# servir_para_el_visor.sh — Servir los ficheros al visor POR RED (sin USB).
#
# El problema: el cable USB de la Pico 4 Ultra es erratico (se ha soltado
# cuatro veces) y a veces no enumera.  La solucion es no necesitar el USB:
#
#   1. En el Mac, este script levanta un servidor HTTP con todo lo que el
#      visor necesita (la APK, el motor, el payload de Mesa, el runtime de
#      glibc, el inyector y el mod).
#   2. En la Pico, el navegador descarga la APK y se instala (hay que
#      permitir "instalar apps de origen desconocido" una vez).
#   3. La app, al arrancar, ve que falta el payload y ejecuta el
#      bootstrap (scripts/bootstrap_visor.sh), que se descarga con el wget
#      de toybox y desempaqueta con su tar.
#
# El motor tiene que vivir en el directorio de datos de la app y correr con
# su uid/gid (es la unica way en que el motor puede tocar la GPU sin root).
#
# Uso:
#     bash scripts/servir_para_el_visor.sh [--puerto 8099] [--solo-indice]
#
# Luego, en el navegador de la Pico:
#     http://<IP-DEL-MAC>:<PUERTO>/

set -uo pipefail

PUERTO=8099
RAIZ="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="$RAIZ/build"
STAGE="$BUILD/red"

while [ $# -gt 0 ]; do
    case "$1" in
        --puerto) PUERTO="${2:-8099}"; shift 2 ;;
        --solo-indice) SOLO=1; shift ;;
        *) shift ;;
    esac
done

say() { printf '%s\n' "$*"; }

# --- 1. Reunir lo que hay que servir ---------------------------------------
say "Preparando lo que se sirve al visor..."
mkdir -p "$STAGE" 2>/dev/null || true

copiar() {
    local origen="$1" destino="$2"
    if [ -f "$origen" ]; then
        cp -f "$origen" "$STAGE/$destino" 2>/dev/null && say "  + $destino"
    else
        say "  ! falta $origen (se salta)"
    fi
}

# La APK (la primera cosa que se instala desde el navegador).
copiar "$BUILD/XFactorVR_Pico4Ultra.apk" "XFactorVR_Pico4Ultra.apk"

# El motor grafico: el paquete ligero (sin audio) del cliente.
if [ -d "$BUILD/factorio_stage_ligero" ]; then
    (cd "$BUILD/factorio_stage_ligero" && tar cf "$STAGE/factorio_cliente.tar" .) 2>/dev/null \
        && say "  + factorio_cliente.tar"
elif [ -f "$BUILD/client_grafico.tar" ]; then
    copiar "$BUILD/client_grafico.tar" "factorio_cliente.tar"
else
    say "  ! falta el motor grafico (bash scripts/deploy_pico.sh lo prepara)"
fi

# El payload de Mesa (el que abre el contexto GL sobre el Adreno).
copiar "$BUILD/mesa.tar.gz" "mesa.tar.gz"

# El runtime de glibc (el motor es un binario glibc y Android es bionic).
copiar "$BUILD/glibc.tar.gz" "glibc.tar.gz"

# El inyector y el mod.
copiar "$RAIZ/injector-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so" \
      "libxfactorvr_injector.so"
if [ -f "$RAIZ/mods/xfactorvr_bridge/control.lua" ]; then
    mkdir -p "$STAGE/mod" 2>/dev/null || true
    cp -f "$RAIZ/mods/xfactorvr_bridge/control.lua" "$STAGE/mod/control.lua" 2>/dev/null \
        && cp -f "$RAIZ/mods/xfactorvr_bridge/info.json" "$STAGE/mod/info.json" 2>/dev/null \
        && say "  + mod/"
fi

# El indice: lo primero que se abre en el navegador de la Pico.
IP=$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || hostname -I 2>/dev/null | awk '{print $1}')
cat > "$STAGE/index.html" <<HTML
<!doctype html><meta charset="utf-8"><title>XFactorVR para la Pico</title>
<style>body{font:16px system-ui;margin:2rem;background:#111;color:#eee}
a{display:block;padding:.6rem 1rem;margin:.4rem 0;background:#1d3b5c;border-radius:8px;color:#cfe8ff;text-decoration:none}</style>
<h1>XFactorVR — instalacion sin USB</h1>
<p>1. Descarga e instala la APK (permite "origenes desconocidos" una vez).</p>
<p>2. Abre la app: si falta el payload, se descarga sola desde aqui.</p>
<p>3. Los ficheros (tambien puedes bajarlos a mano):</p>
<a href="XFactorVR_Pico4Ultra.apk">APK del visor</a>
<a href="factorio_cliente.tar">Motor grafico (sin audio)</a>
<a href="mesa.tar.gz">Payload de Mesa (el que abre GL sobre el Adreno)</a>
<a href="glibc.tar.gz">Runtime de glibc</a>
<a href="libxfactorvr_injector.so">Inyector</a>
<a href="mod/control.lua">Mod puente (control.lua)</a>
<p style="color:#888">Servido desde $IP:$PUERTO</p>
HTML

say ""
say "Listo. En el navegador de la Pico abre:"
say "    http://$IP:$PUERTO/"
say ""
say "El motor grafico son $(du -sh "$STAGE" 2>/dev/null | cut -f1) en total."

if [ "${SOLO:-0}" = "1" ]; then
    exit 0
fi

say "Levantando el servidor (Ctrl+C para parar)..."
cd "$STAGE" && exec python3 -m http.server "$PUERTO" --bind 0.0.0.0
