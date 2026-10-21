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
MOD_NAME="xfactorvr_bridge"
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

# ---------------------------------------------------------------------------
# 4 bis. Ajustes del servidor: la simulacion debe correr SIN jugadores
# ---------------------------------------------------------------------------
# Factorio pausa el juego mientras no haya nadie conectado (auto-pause). En un
# servidor headless no hay cliente nunca, asi que con el valor por defecto la
# fabrica se queda CONGELADA: el visor veria un mundo que no avanza. Hay que
# desactivarlo explicitamente.
#
# El motor lee los ajustes de servidor del directorio de configuracion, que
# con config-path.cfg es <raiz>/config (config-path=__PATH__executable__/../../config).
CONFIG_DIR="$FACTORIO_DIR/config"
SETTINGS_FILE="$CONFIG_DIR/server-settings.json"
mkdir -p "$CONFIG_DIR" 2>/dev/null || true

# El motor deduce sus rutas (read-data / write-data) de /proc/self/exe.  En
# Android se arranca a traves del linker de glibc (/data/local/tmp/glibc/lib),
# asi que /proc/self/exe apunta AL LINKER y el motor busca el juego en
# /usr/share/factorio: "There is no package core".  Se le da un config.ini
# propio con las rutas en absoluto, igual que hace el cliente grafico.
SERVER_CONFIG="$CONFIG_DIR/config.ini"
if [ ! -f "$SERVER_CONFIG" ]; then
    log "Creando el config.ini del servidor (rutas absolutas)..."
    cat > "$SERVER_CONFIG" <<CFG
[path]
read-data=$FACTORIO_DIR/data
write-data=$FACTORIO_DIR

[other]
check-updates=false
CFG
fi

# En modo CAMPANA el auto_pause va DESACTIVADO.
#
# Antes ponia true porque el nivel de campana espera a game.players[1] (que
# solo existe si hay un cliente grafico conectado) y sin congelar el mapa el
# nivel se moria antes de que llegara.  Ahora el mod crea un JUGADOR VIRTUAL
# (una entidad "character" con el tag xfactorvr_player) y el escenario
# parcheado usa ese jugador, asi que el nivel ya no espera a nadie: si el
# mapa esta congelado, el tutorial no avanza nunca.
#
# Con auto_pause=false el nivel avanza solo desde el primer tick, y el
# jugador virtual lo juega desde el visor por RCON.
if [ -n "${SCENARIO:-}" ]; then
    log "Modo campana: auto_pause=false (el jugador virtual permite avanzar)"
    cat > "$SETTINGS_FILE" <<'JSON'
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
elif [ ! -f "$SETTINGS_FILE" ]; then
    log "Creando $SETTINGS_FILE con auto_pause desactivado..."
    cp /data/local/tmp/server-settings.json "$SETTINGS_FILE" 2>/dev/null \
        || cat > "$SETTINGS_FILE" <<'JSON'
{
  "name": "XFactorVR",
  "description": "Factorio para VR",
  "visibility": { "public": false, "lan": true },
  "require_user_verification": false,
  "auto_pause": false,
  "auto_pause_when_players_connect": false,
  "autosave_interval": 10,
  "autosave_slots": 3
}
JSON
else
    # Si existe, se asegura de que auto_pause siga desactivado.
    if ! grep -q '"auto_pause"[[:space:]]*:[[:space:]]*false' "$SETTINGS_FILE" 2>/dev/null; then
        log "Forzando auto_pause=false en $SETTINGS_FILE"
        cp "$SETTINGS_FILE" "$SETTINGS_FILE.bak" 2>/dev/null || true
        cat > "$SETTINGS_FILE" <<'JSON'
{
  "name": "XFactorVR",
  "description": "Factorio para VR",
  "visibility": { "public": false, "lan": true },
  "require_user_verification": false,
  "auto_pause": false,
  "auto_pause_when_players_connect": false,
  "autosave_interval": 10,
  "autosave_slots": 3
}
JSON
    fi
fi
log "Ajustes de servidor: $SETTINGS_FILE"

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
# La carpeta se localiza por prefijo: la version va en el nombre de la carpeta
# y cambia con cada entrega, asi que no se fija a mano.
MOD_PATH=""
for _d in "$MOD_DIR/$MOD_NAME"_*/; do
    if [ -f "${_d}control.lua" ] && [ -f "${_d}info.json" ]; then
        MOD_PATH="${_d%/}"
        break
    fi
done

if [ -n "$MOD_PATH" ]; then
    log "Mod puente presente: $MOD_PATH"
else
    log "AVISO: no se encuentra el mod $MOD_NAME en $MOD_DIR."
    log "        El servidor arrancara, pero sin puente RCON el juego"
    log "        seguira usando la simulacion local de Rust."
fi

# ---------------------------------------------------------------------------
# 4 ter. Resolutor de nombres
# ---------------------------------------------------------------------------
# El motor es un binario de GLIBC y Android no tiene /etc/resolv.conf (la
# resolucion la hace netd por binder).  Asi que dentro del motor no se
# resuelve NINGUN nombre, y el servidor multijugador cae al pedir el
# "server padlock" a auth.factorio.com:
#
#   CURL failed: code:6, 6; Could not resolve host: auth.factorio.com
#
# El shim intercepta getaddrinfo y, si la glibc falla, resuelve el nombre con
# una consulta DNS por UDP propia.
RESOLVER="${XFACTORVR_RESOLVER:-/data/local/tmp/libxfactorvr_resolver.so}"
# El curl del motor va enlazado estatico, con su propia resolucion de
# nombres (busca /etc/resolv.conf, que en Android no existe, y usa
# 127.0.0.1 por defecto, donde no hay nada: la peticion del padlock se
# queda 60 s y el servidor multijugador no arranca).  curl si lee las
# variables del proxy, asi que se le apunta al proxy CONNECT que trae el
# shim, que resuelve los nombres de verdad.
PROXY_PUERTO="${XFACTORVR_PROXY_PUERTO:-8899}"
export https_proxy="http://127.0.0.1:$PROXY_PUERTO"
export http_proxy="http://127.0.0.1:$PROXY_PUERTO"

PRELOAD=""
if [ -f "$RESOLVER" ]; then
    PRELOAD="$RESOLVER"
    log "Resolutor de nombres: $RESOLVER"
else
    log "AVISO: no esta el resolutor de nombres en $RESOLVER."
    log "       El servidor podria no arrancar (no puede resolver"
    log "       auth.factorio.com para el padlock del modo multijugador)."
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
    # Ruta ABSOLUTA: con un nombre relativo el motor la crea en el directorio
    # de trabajo (la raiz del juego) y luego el script no la encuentra en
    # saves/ y da la partida por perdida.
    ( cd "$FACTORIO_DIR" && \
        env LD_PRELOAD="$PRELOAD" "$LOADER" --library-path "$GLIBC_LIB" "$FACTORIO_BIN" \
            --config "$SERVER_CONFIG" \
        --create "$SAVE_DIR/$SAVE_NAME" --mod-directory "$MOD_DIR" ) >>"$LOG_FILE" 2>&1

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
#
# Modo CAMPANA (SCENARIO=base/<campaña>): el servidor carga un escenario de
# campaña en vez de una partida guardada. La campana necesita un JUGADOR real
# (el nivel usa game.players[1]), asi que en este modo el cliente grafico se
# conecta despues con --mp-connect. Y necesita auto_pause=true: con false el
# mapa avanza antes de que llegue el jugador y el nivel muere.
launch() {
    cd "$FACTORIO_DIR" || return 1
    if [ -n "${SCENARIO:-}" ]; then
        log "Arrancando el escenario de campana: $SCENARIO"
        nohup \
            env LD_PRELOAD="$PRELOAD" "$LOADER" --library-path "$GLIBC_LIB" "$FACTORIO_BIN" \
            --config "$SERVER_CONFIG" \
            --start-server-load-scenario "$SCENARIO" \
            --port "${SERVER_PORT:-34197}" \
            --rcon-port "$RCON_PORT" \
            --rcon-password "$RCON_PASSWORD" \
            --mod-directory "$MOD_DIR" \
            >>"$LOG_FILE" 2>&1 &
    else
        nohup \
            env LD_PRELOAD="$PRELOAD" "$LOADER" --library-path "$GLIBC_LIB" "$FACTORIO_BIN" \
            --config "$SERVER_CONFIG" \
            --start-server "$SAVE_FILE" \
            --rcon-port "$RCON_PORT" \
            --rcon-password "$RCON_PASSWORD" \
            --mod-directory "$MOD_DIR" \
            >>"$LOG_FILE" 2>&1 &
    fi
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
        SERVER_PID=$(cd "$FACTORIO_DIR" && nohup \
            env LD_PRELOAD="$PRELOAD" "$LOADER" --library-path "$GLIBC_LIB" "$FACTORIO_BIN" \
            --config "$SERVER_CONFIG" \
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
