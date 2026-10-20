#!/bin/bash
# start_factorio_client.sh — Arranca el cliente GRAFICO de Factorio ARM64 en el
# visor Pico 4 Ultra usando Mesa con Zink (OpenGL 3.3 de escritorio sobre el
# driver Vulkan de Adreno).
#
# Por que Zink: el cliente Linux de Factorio pide OpenGL 3.3 core. Android no
# tiene OpenGL de escritorio, pero Zink (Mesa) traduce OpenGL a Vulkan, de modo
# que el juego corre sobre el driver Vulkan del propio Adreno (aceleracion por
# hardware) en vez de por software.
#
# El cliente trae SDL2 estatico dentro del binario y carga la capa grafica en
# tiempo de ejecucion con dlopen de libGL.so.1 / libEGL.so.1 / libvulkan.so.1,
# asi que basta conLeaks del payload de Mesa y las variables de entorno.
#
# Requiere: el payload de Mesa (scripts/prepare_mesa.py) y el cliente grafico
# (factorio_linux-arm64_2.1.22.tar.xz) en /data/local/tmp.

set -uo pipefail

FACTORIO_DIR="${FACTORIO_DIR:-/data/local/tmp/factorio_client}"
MESA_DIR="${MESA_DIR:-/data/local/tmp/mesa}"
LOG_DIR="${LOG_DIR:-$FACTORIO_DIR/logs}"
RCON_PORT="${RCON_PORT:-27015}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
WIDTH="${WIDTH:-1280}"
HEIGHT="${HEIGHT:-720}"

log() { echo "[factorio-client] $*"; }

# --- 1. Comprobar el payload ------------------------------------------------
if [ ! -f "$MESA_DIR/lib/ld-linux-aarch64.so.1" ]; then
    log "ERROR: no esta el payload de Mesa en $MESA_DIR"
    log "       genera con: python3 scripts/prepare_mesa.py"
    exit 1
fi
if [ ! -f "$FACTORIO_DIR/bin/arm64/factorio" ]; then
    log "ERROR: no esta el cliente grafico en $FACTORIO_DIR/bin/arm64/factorio"
    exit 1
fi

MESA_LIB="$MESA_DIR/usr/lib/aarch64-linux-gnu"
LOADER="$MESA_DIR/lib/ld-linux-aarch64.so.1"

# --- 2. Localizar el ICD de Vulkan de Android (el driver de Adreno) ---------
# Android guarda los ICD del fabricante en /vendor/lib64/hw/vulkan. El nombre
# del fichero cambia entre fabricantes y versiones, asi que se busca.
VENDOR_VULKAN_DIR="${VENDOR_VULKAN_DIR:-/vendor/lib64/hw/vulkan}"
ICD_JSON="$MESA_DIR/adreno_icd.json"

if [ ! -f "$ICD_JSON" ]; then
    ICD_SO=""
    for cand in "$VENDOR_VULKAN_DIR"/libvulkan_adreno.so \
                "$VENDOR_VULKAN_DIR"/vulkan.adreno.so \
                "$VENDOR_VULKAN_DIR"/libvulkan_qcom.so \
                "$VENDOR_VULKAN_DIR"/*.so; do
        if [ -f "$cand" ]; then
            ICD_SO="$cand"
            break
        fi
    done
    if [ -n "$ICD_SO" ]; then
        log "ICD de Vulkan encontrado: $ICD_SO"
        cat > "$ICD_JSON" <<JSON
{
    "file_format_version": "1.0.0",
    "ICD": {
        "library_path": "$ICD_SO",
        "api_version": "1.3.239"
    }
}
JSON
    else
        log "AVISO: no se encontro un ICD de Vulkan en $VENDOR_VULKAN_DIR"
        log "       se usara lavapipe (Vulkan por software) si esta disponible"
    fi
fi

# --- 3. Entorno de Mesa -----------------------------------------------------
export LIBGL_DRIVERS_PATH="$MESA_LIB/dri"
export MESA_LOADER_DRIVER_OVERRIDE=zink
export ZINK_DESCRIPTORS=auto
export GALLIUM_DRIVER=zink
export LIBGL_ALWAYS_SOFTWARE=0
export LD_LIBRARY_PATH="$MESA_LIB:$MESA_DIR/lib:$MESA_DIR/lib/aarch64-linux-gnu:$LD_LIBRARY_PATH"

if [ -f "$ICD_JSON" ]; then
    export VK_ICD_FILENAMES="$ICD_JSON"
    export VK_DRIVER_FILES="$ICD_JSON"
else
    # Sin ICD de Adreno: lavapipe (Vulkan por software) para que al menos arranque.
    if [ -f "$MESA_DIR/usr/share/vulkan/icd.d/lvp_icd.aarch64.json" ]; then
        export VK_ICD_FILENAMES="$MESA_DIR/usr/share/vulkan/icd.d/lvp_icd.aarch64.json"
    fi
fi

# SDL: la transicion de estado del motor (cargar partida, abrir menus) toca el
# GUI, asi que con el driver "offscreen" se queda colgado. Si hay un servidor X
# disponible (termux-x11 en Android, Xvfb en el banco), se usa; si no, se
# intenta igual con offscreen.
if [ -z "${DISPLAY:-}" ]; then
    # Busca un X de termux-x11 o Xvfb si hay alguno vivo.
    for d in :99 :0 :1; do
        if [ -e "/tmp/.X11-unix/X${d#:}" ] || [ -S "/tmp/.X11-unix/X${d#:}" ]; then
            export DISPLAY="$d"
            break
        fi
    done
fi
if [ -n "${DISPLAY:-}" ]; then
    export SDL_VIDEODRIVER="${SDL_VIDEODRIVER:-x11}"
    log "servidor X: $DISPLAY (driver ${SDL_VIDEODRIVER})"
else
    export SDL_VIDEODRIVER="${SDL_VIDEODRIVER:-offscreen}"
    log "sin servidor X: driver ${SDL_VIDEODRIVER} (el cambio de estado puede fallar)"
fi
export SDL_AUDIODRIVER="${SDL_AUDIODRIVER:-dummy}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/data/local/tmp/xdg-runtime}"
mkdir -p "$XDG_RUNTIME_DIR" 2>/dev/null || true

# --- Inyector del mod 3D -------------------------------------------------
# El .so engancha el pipeline de render del juego (resuelve los hooks por
# nombre desde el symtab del ELF) y es lo que permite arrancar la partida y
# dibujar el mundo en 3D. Es opcional: sin el, el juego funciona igual.
INJECTOR="${INJECTOR:-/data/local/tmp/libxfactorvr_injector.so}"
if [ -f "$INJECTOR" ] && [ "${XFACTORVR_USE_INJECTOR:-1}" = "1" ]; then
    export LD_PRELOAD="$INJECTOR${LD_PRELOAD:+:$LD_PRELOAD}"
    log "inyector: $INJECTOR"
    # Destino de los hooks: por defecto el render del juego.
    export XFACTORVR_HOOK_TARGET="${XFACTORVR_HOOK_TARGET:-GameRenderer::render}"
    export XFACTORVR_HOOK_EVERY="${XFACTORVR_HOOK_EVERY:-60}"
fi

# --- 4. Estado inicial ------------------------------------------------------
mkdir -p "$LOG_DIR" "$FACTORIO_DIR/config" "$FACTORIO_DIR/saves" 2>/dev/null || true
if [ ! -f "$FACTORIO_DIR/config/config-path.cfg" ]; then
    cat > "$FACTORIO_DIR/config/config-path.cfg" <<CFG
config-path=$FACTORIO_DIR/config
use-system-read-write-data-directories=false
CFG
fi
# La simulacion debe correr sin jugadores (ver scripts/server-settings.json).
if [ ! -f "$FACTORIO_DIR/config/server-settings.json" ]; then
    cat > "$FACTORIO_DIR/config/server-settings.json" <<'JSON'
{
  "name": "XFactorVR",
  "description": "Factorio para Realidad Mixta",
  "visibility": { "public": false, "lan": true },
  "require_user_verification": false,
  "auto_pause": false,
  "auto_pause_when_players_connect": false,
  "autosave_interval": 10,
  "autosave_slots": 3
}
JSON
fi

# --- 5. Arranque ------------------------------------------------------------
GAME_BIN="$FACTORIO_DIR/bin/arm64/factorio"
SAVE="${SAVE:-$FACTORIO_DIR/saves/xfactorvr.zip}"

if [ ! -f "$SAVE" ]; then
    log "Creando la partida inicial..."
    "$LOADER" "$GAME_BIN" --create "$SAVE" --config "$FACTORIO_DIR/config/config-path.cfg" \
        2>&1 | tail -20
fi

log "Arrancando el cliente grafico (Zink -> Vulkan) en $WIDTH x $HEIGHT..."
log "  ICD: ${VK_ICD_FILENAMES:-ninguno}"
log "  SDL: $SDL_VIDEODRIVER"

exec "$LOADER" "$GAME_BIN" \
    --config "$FACTORIO_DIR/config/config-path.cfg" \
    --width "$WIDTH" --height "$HEIGHT" \
    --fullscreen \
    --rcon-port "$RCON_PORT" \
    --rcon-password "$RCON_PASSWORD" \
    "$SAVE"
