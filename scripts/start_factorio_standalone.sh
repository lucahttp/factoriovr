#!/bin/sh
#
# start_factorio_standalone.sh
#
# Arranca el servidor headless de Factorio 2.0 DENTRO del visor Pico 4 Ultra.
# Esto es lo que convierte la aplicacion de "demo con simulacion local" en el
# juego real: el motor de Factorio corre en el dispositivo y la mesa OpenXR
# refleja su estado real por RCON.
#
# Restricciones del dispositivo (Pico OS / Android):
#   - /sdcard/ esta montado como noexec: el binario NO puede ejecutarse ahi,
#     por eso todo el motor vive en /data/local/tmp/.
#   - Android usa bionic y el binario ARM64 de Factorio exige glibc, asi que
#     hace falta el linker de glibc (Termux glibc o un rootfs copiado a mano).
#
# Uso (en el visor, via adb shell):
#   sh /data/local/tmp/start_factorio_standalone.sh
#
# Variables de entorno opcionales:
#   FACTORIO_DIR   raiz del motor    (por defecto /data/local/tmp/factorio_arm64)
#   RCON_PORT      puerto RCON        (por defecto 27015)
#   RCON_PASSWORD  contrasena RCON    (por defecto xfactorvr)
#   SAVE_NAME      partida            (por defecto xfactorvr_world.zip)
#   WAIT_TIMEOUT   segundos de espera (por defecto 120)

set -u

FACTORIO_DIR="${FACTORIO_DIR:-/data/local/tmp/factorio_arm64}"
FACTORIO_BIN="$FACTORIO_DIR/bin/arm64/factorio"
MOD_DIR="$FACTORIO_DIR/mods"
MOD_ID="xfactorvr_bridge_0.1.0"
SAVE_NAME="${SAVE_NAME:-xfactorvr_world.zip}"
RCON_PORT="${RCON_PORT:-27015}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
WAIT_TIMEOUT="${WAIT_TIMEOUT:-120}"
SAVE_DIR="$FACTORIO_DIR/saves"
LOG_FILE="$FACTORIO_DIR/logs/standalone.log"
PID_FILE="$FACTORIO_DIR/factorio.pid"

log()  { echo "[factorio] $*"; }
fail() { echo "[factorio][ERROR] $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1. ¿Hay ya un servidor en marcha?
# ---------------------------------------------------------------------------
# pidof no esta en todas las imagenes de Android, asi que se busca tambien
# en /proc. Un servidor vivo significa que no hay que levantar otro: dos
#_factorio en el mismo puerto RCON se pelean por el estado del mundo.
existing_pid() {
    _pid=$(pidof factorio 2>/dev/null)
    if [ -n "$_pid" ]; then
        printf '%s' "$_pid"
        return 0
    fi
    for _p in /proc/[0-9]*; do
        [ -r "$_p/comm" ] || continue
        if [ "$(cat "$_p/comm" 2>/dev/null)" = "factorio" ]; then
            printf '%s' "${_p#/proc/}"
            return 0
        fi
    done
    return 1
}

# ---------------------------------------------------------------------------
# 2. ¿El puerto RCON ya escucha?
# ---------------------------------------------------------------------------
port_open() {
    _hex=$(printf '%04X' "$RCON_PORT")
    grep -qi ":${_hex} " /proc/net/tcp 2>/dev/null && return 0
    grep -qi ":${_hex} " /proc/net/tcp6 2>/dev/null && return 0
    return 1
}

# ---------------------------------------------------------------------------
# 3. Localizar el linker de glibc
# ---------------------------------------------------------------------------
find_loader() {
    for _c in \
        "/data/local/tmp/glibc/lib/ld-linux-aarch64.so.1" \
        "/data/local/tmp/glibc/lib64/ld-linux-aarch64.so.1" \
        "$FACTORIO_DIR/glibc/lib/ld-linux-aarch64.so.1" \
        "/data/data/com.termux/files/usr/glibc/lib/ld-linux-aarch64.so.1" \
        "/data/data/com.termux/files/usr/lib/ld-linux-aarch64.so.1"
    do
        if [ -x "$_c" ]; then
            printf '%s' "$_c"
            return 0
        fi
    done
    return 1
}

# ---------------------------------------------------------------------------
# 4. Estado inicial
# ---------------------------------------------------------------------------
mkdir -p "$FACTORIO_DIR/logs" 2>/dev/null || true

if existing_pid >/dev/null 2>&1; then
    _pid=$(existing_pid)
    log "Ya hay un servidor Factorio en marcha (PID $_pid)."
    if port_open; then
        log "RCON escuchando en 127.0.0.1:$RCON_PORT. No se toca nada."
        exit 0
    fi
    log "El proceso existe pero el puerto RCON aun no escucha; se espera..."
    _waited=0
    while [ "$_waited" -lt "$WAIT_TIMEOUT" ]; do
        port_open && { log "RCON listo en 127.0.0.1:$RCON_PORT."; exit 0; }
        sleep 1
        _waited=$((_waited + 1))
    done
    fail "El servidor sigue sin abrir el puerto RCON. Revisa $LOG_FILE"
fi

[ -x "$FACTORIO_BIN" ] || fail "No se encuentra el motor en $FACTORIO_BIN.
Ejecuta primero scripts/deploy_pico.sh para copiar Factorio al visor."

LOADER=$(find_loader) || fail "No se encuentra un linker de glibc.
Factorio ARM64 es un binario de Linux y Android usa bionic: hace falta glibc.
Opciones:
  - Termux instalado:  pkg install glibc-repo && pkg install glibc
  - O bien ejecuta en el PC:  bash scripts/prepare_glibc.sh
    y copia el resultado a /data/local/tmp/glibc/lib/"

GLIBC_LIB=$(dirname "$LOADER")
log "Linker de glibc: $LOADER"

# El mod puente es lo que publica el estado del mundo por RCON. Si no esta,
# el servidor arranca pero el juego seguira viendo solo la simulacion local.
if [ -f "$MOD_DIR/$MOD_ID/control.lua" ] && [ -f "$MOD_DIR/$MOD_ID/info.json" ]; then
    log "Mod puente presente: $MOD_DIR/$MOD_ID"
else
    log "AVISO: no se encuentra el mod $MOD_ID en $MOD_DIR."
    log "        El servidor arrancara, pero sin puente RCON el juego"
    log "        seguira usando la simulacion local de Rust."
fi

# ---------------------------------------------------------------------------
# 5. Crear la partida si no existe
# ---------------------------------------------------------------------------
# Con use-system-read-write-data-directories=false el motor usa su propia raiz
# como directorio de escritura, asi que las partidas viven en saves/.
SAVE_FILE=""
if [ -f "$SAVE_DIR/$SAVE_NAME" ]; then
    SAVE_FILE="$SAVE_DIR/$SAVE_NAME"
elif [ -f "$SAVE_DIR/${SAVE_NAME%.zip}.zip" ]; then
    SAVE_FILE="$SAVE_DIR/${SAVE_NAME%.zip}.zip"
fi

if [ -z "$SAVE_FILE" ]; then
    log "Creando la partida inicial ($SAVE_NAME)..."
    mkdir -p "$SAVE_DIR"
    ( cd "$FACTORIO_DIR" && LD_LIBRARY_PATH="$GLIBC_LIB" \
        "$LOADER" --library-path "$GLIBC_LIB" "$FACTORIO_BIN" \
        --create "$SAVE_NAME" --mod-directory "$MOD_DIR" ) >>"$LOG_FILE" 2>&1

    for _cand in "$SAVE_DIR/$SAVE_NAME" "$SAVE_DIR/${SAVE_NAME%.zip}.zip"; do
        [ -f "$_cand" ] && SAVE_FILE="$_cand" && break
    done

    if [ -z "$SAVE_FILE" ]; then
        fail "No se pudo crear la partida. Ultimas lineas de $LOG_FILE:"
        tail -n 25 "$LOG_FILE" >&2 2>/dev/null || true
    fi
    log "Partida creada: $SAVE_FILE"
else
    log "Partida existente: $SAVE_FILE"
fi

# ---------------------------------------------------------------------------
# 6. Arrancar el servidor
# ---------------------------------------------------------------------------
# Se lanza con nohup para que sobreviva al cierre de la shell de adb: el
# servidor debe seguir corriendo con el visor desconectado del PC.
launch() {
    cd "$FACTORIO_DIR" || return 1
    LD_LIBRARY_PATH="$GLIBC_LIB" nohup \
        "$LOADER" --library-path "$GLIBC_LIB" "$FACTORIO_BIN" \
        --start-server "$SAVE_FILE" \
        --rcon-port "$RCON_PORT" \
        --rcon-password "$RCON_PASSWORD" \
        --mod-directory "$MOD_DIR" \
        >>"$LOG_FILE" 2>&1 &
    echo $!
}

log "Arrancando el servidor Factorio (RCON en 127.0.0.1:$RCON_PORT)..."
SERVER_PID=$(launch)
echo "$SERVER_PID" > "$PID_FILE"
log "PID del servidor: $SERVER_PID"

# ---------------------------------------------------------------------------
# 7. Esperar a que el puerto RCON abra
# ---------------------------------------------------------------------------
_waited=0
while [ "$_waited" -lt "$WAIT_TIMEOUT" ]; do
    if port_open; then
        log "Servidor Factorio listo: RCON en 127.0.0.1:$RCON_PORT (${_waited}s)"
        log "Registro: $LOG_FILE"
        exit 0
    fi
    # Si el proceso murio se aborta: no tiene sentido esperar a un servidor
    # que ya no existe.
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        log "El proceso del servidor termino. Ultimas lineas de $LOG_FILE:"
        tail -n 25 "$LOG_FILE" >&2 2>/dev/null || true
        # Reintento con la variante --start-server-load-latest: la aceptacion
        # de una ruta absoluta de partida depende de la version del motor.
        log "Reintentando con --start-server-load-latest..."
        SERVER_PID=$(cd "$FACTORIO_DIR" && LD_LIBRARY_PATH="$GLIBC_LIB" nohup \
            "$LOADER" --library-path "$GLIBC_LIB" "$FACTORIO_BIN" \
            --start-server-load-latest \
            --rcon-port "$RCON_PORT" \
            --rcon-password "$RCON_PASSWORD" \
            --mod-directory "$MOD_DIR" \
            >>"$LOG_FILE" 2>&1 & echo $!)
        echo "$SERVER_PID" > "$PID_FILE"
        _waited=0
        while [ "$_waited" -lt 30 ]; do
            port_open && { log "Servidor listo con --start-server-load-latest."; exit 0; }
            sleep 1
            _waited=$((_waited + 1))
        done
        fail "El servidor no abrio el puerto RCON. Revisa $LOG_FILE"
    fi
    sleep 1
    _waited=$((_waited + 1))
done

fail "Tiempo agotado esperando el puerto RCON $RCON_PORT. Revisa $LOG_FILE"
