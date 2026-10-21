#!/data/data/com.termux/files/usr/bin/sh
# start_factorio_termux.sh — Factorio ARM64 dentro de Termux (Pico 4 Ultra).
#
# POR QUE TERMUX Y NO /data/local/tmp
#
# El motor de Factorio es un binario de **glibc** y hace dlopen de libGL.so.1
# para crear el contexto OpenGL.  En Android, un proceso del shell (el de
# /data/local/tmp) no puede hacer dlopen de una libreria de ese sitio:
#
#   Could not initialize OpenGL / GLES library
#
# porque SELinux no da permiso de execute_no_trans sobre shell_data_file.  Y
# tampoco vale la pena meter Mesa a mano: el driver Vulkan del Adreno
# (/vendor/lib64/hw/vulkan.adreno.so) es un .so de BIONIC, asi que un proceso
# glibc no puede cargarlo (dos libc en el mismo proceso).
#
# Dentro de Termux el problema desaparece de dos maneras:
#   - el proceso es bionic (nativo), asi que el X server de Termux:X11 le
#     da el contexto GL por el socket X11, sin dlopen de nada raro;
#   - Termux tiene su propio paquete "glibc" (el mismo que usa `pkg install
#     glibc`), y el binario ARM64 de Factorio arranca con su loader.
#
# Es la via que describe la comunidad (Termux + Termux:X11).
#
# REQUISITOS (una vez):
#   pkg install x11-repo
#   pkg install termux-x11-nightly     # o termux-x11
#   pkg install glibc
#   termux-x11 &                       # servidor X en :99
#
# Y el motor dentro de Termux: se copia de /sdcard/XFactorVR (el despliegue lo
# deja ahi) a $HOME/factorio, porque /sdcard es noexec.
#
# USO:
#   sh ~/start_factorio_termux.sh            # servidor + cliente
#   sh ~/start_factorio_termux.sh cliente    # solo el cliente grafico
#
set -u

PREFIX="/data/data/com.termux/files/usr"
HOME_DIR="$PREFIX/home"
RAIZ="$HOME_DIR/factorio"
# Donde deja el motor el despliegue.  /sdcard es noexec, asi que el motor se
# copia al home de Termux (el proceso de Termux tampoco puede leer
# /data/local/tmp por SELinux).
ORIGEN="/sdcard/XFactorVR/ligero"
MESA_LIB="$PREFIX/glibc/lib"
LOADER="$MESA_LIB/ld-linux-aarch64.so.1"
BIN="$RAIZ/bin/arm64/factorio"
MODS="$RAIZ/mods"
LOG_DIR="$HOME_DIR/logs"
RCON_PORT=27015
RCON_PASSWORD=xfactorvr
SERVER_PORT=34197

log() { echo "[fx-termux] $*"; }

mkdir -p "$LOG_DIR" "$MODS" "$RAIZ/saves"

# --- 1. El motor tiene que estar dentro de Termux -------------------------
if [ ! -x "$BIN" ]; then
    if [ -f "$ORIGEN/bin/arm64/factorio" ]; then
        log "Copiando el motor desde /sdcard (la primera vez tarda un poco)..."
        mkdir -p "$RAIZ"
        cp -R "$ORIGEN/." "$RAIZ/"
        chmod 755 "$BIN"
    else
        log "ERROR: no hay motor. Deja el juego en $ORIGEN"
        log "       (lo pone el despliegue: bash scripts/deploy_pico.sh --campana)"
        exit 1
    fi
fi
[ -f "$LOADER" ] || { log "ERROR: falta glibc. Ejecuta: pkg install glibc"; exit 1; }

# --- 2. El servidor X de Termux:X11 ---------------------------------------
if [ -z "${DISPLAY:-}" ]; then
    for d in :99 :0 :1; do
        if [ -S "/tmp/.X11-unix/X${d#:}" ]; then
            export DISPLAY="$d"
            break
        fi
    done
fi
if [ -z "${DISPLAY:-}" ]; then
    log "ERROR: no hay servidor X. Arranca uno:  termux-x11 &"
    exit 1
fi
log "display: $DISPLAY"

# --- 3. Config.ini con rutas en absoluto -----------------------------------
CFG="$RAIZ/config/config.ini"
mkdir -p "$RAIZ/config"
cat > "$CFG" <<EOF
[path]
read-data=$RAIZ/data
write-data=$RAIZ

[other]
check-updates=false
EOF

# --- 4. Servidor headless (el que tiene el RCON) --------------------------
if [ "${1:-}" != "cliente" ]; then
    if ! (exec 3<>/dev/tcp/127.0.0.1/$RCON_PORT) 2>/dev/null; then
        log "Arrancando el servidor Factorio headless..."
        nohup "$LOADER" --library-path "$MESA_LIB" "$BIN" \
            --config "$CFG" \
            --start-server "$RAIZ/saves/xfactorvr_world.zip" \
            --rcon-port "$RCON_PORT" \
            --rcon-password "$RCON_PASSWORD" \
            --mod-directory "$MODS" \
            >>"$LOG_DIR/server.log" 2>&1 &
        sleep 8
    else
        log "Ya hay un servidor escuchando en $RCON_PORT"
    fi
fi

# --- 5. Cliente grafico: es el que aporta el jugador que la campana exige -
#
# El cliente lleva el INYECTOR: es el proceso donde se engancha el modulo 3D
# (el cubo de las entidades) y el que manda el render real del juego al
# visor VR por TCP (protocolo XFV1).  El APK escucha en 127.0.0.1:27020.
INYECTOR="$HOME_DIR/libxfactorvr_injector.so"
if [ ! -f "$INYECTOR" ]; then
    if [ -f "/sdcard/XFactorVR/libxfactorvr_injector.so" ]; then
        log "Copiando el inyector desde /sdcard..."
        cp "/sdcard/XFactorVR/libxfactorvr_injector.so" "$INYECTOR"
    else
        log "AVISO: no hay inyector en /sdcard/XFactorVR: no habra 3D ni render real"
    fi
fi

export XFACTORVR_HOOK_TARGET="${XFACTORVR_HOOK_TARGET:-GameRenderer::render}"
export XFACTORVR_HOOK_EVERY="${XFACTORVR_HOOK_EVERY:-60}"
export XFACTORVR_3D="${XFACTORVR_3D:-1}"
# El visor VR (el APK) escucha en 127.0.0.1:27020 y pinta lo que llega.
export XFACTORVR_STREAM="${XFACTORVR_STREAM:-127.0.0.1:27020}"
export XFACTORVR_STREAM_ANCHO="${XFACTORVR_STREAM_ANCHO:-640}"
export XFACTORVR_STREAM_ALTO="${XFACTORVR_STREAM_ALTO:-360}"
export XFACTORVR_STREAM_FPS="${XFACTORVR_STREAM_FPS:-10}"

log "Arrancando el cliente grafico (el jugador del nivel de campana)..."
if [ -f "$INYECTOR" ]; then
    log "inyector: $INYECTOR (render real al visor en $XFACTORVR_STREAM)"
    LD_PRELOAD="$INYECTOR" "$LOADER" --library-path "$MESA_LIB" "$BIN" \
        --config "$CFG" \
        --window-size 1280x720 \
        --mp-connect "127.0.0.1:$SERVER_PORT" \
        >>"$LOG_DIR/client.log" 2>&1 &
else
    "$LOADER" --library-path "$MESA_LIB" "$BIN" \
        --config "$CFG" \
        --window-size 1280x720 \
        --mp-connect "127.0.0.1:$SERVER_PORT" \
        >>"$LOG_DIR/client.log" 2>&1 &
fi
log "Cliente lanzado. Log en $LOG_DIR/client.log"
