#!/bin/sh
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

# Android no tiene bash: el script va con sh (dash).
set -u

FACTORIO_DIR="${FACTORIO_DIR:-/data/local/tmp/factorio}"
MESA_DIR="${MESA_DIR:-/data/local/tmp/mesa}"
LOG_DIR="${LOG_DIR:-$FACTORIO_DIR/logs}"
# El write-data DEL CLIENTE.  Se declara aqui (y no mas abajo) porque el
# script corre con "set -u" y se usa antes: el log del resolutor de nombres
# y la FIFO de la consola cuelgan de aqui, y este directorio es el unico
# que puede escribir el usuario shell (el del servidor es de root).
CLIENT_DIR="${CLIENT_DIR:-/data/local/tmp/factorio_cliente}"
RCON_PORT="${RCON_PORT:-27015}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
# OJO: el tamano de la ventana importa.  Medido en el visor Pico 4 Ultra:
# con 1280x720 el motor se queda ESPERADO despues de "Factorio initialised"
# (nunca llega a "Joining game"); con 640x360 entra en juego del tiron
# ("to(InGame)" + "PlayerJoinGame mode(connect)").
#
# 640x360 es ademas lo que manda el inyector al visor (XFV1), asi que el
# panel del motor tiene el tamano justo de la imagen que se ve.
WIDTH="${WIDTH:-640}"
HEIGHT="${HEIGHT:-360}"

log() { echo "[factorio-client] $*"; }

# --- 1. Comprobar el payload ------------------------------------------------
MESA_LIB="$MESA_DIR/usr/lib/aarch64-linux-gnu"
# El loader y la libc son los del runtime de glibc del visor (Termux 2.44).
# El payload de Mesa NO lleva glibc: si se usara la de Debian (2.36), el
# cliente grafico pediria GLIBC_2.38 y no la encontraria.
GLIBC_DIR="${GLIBC_DIR:-/data/local/tmp/glibc}"
LOADER="$GLIBC_DIR/lib/ld-linux-aarch64.so.1"
GLIBC_LIB="$GLIBC_DIR/lib"

# Camino de busqueda de librerias. IMPORTANTE: sin la carpeta de Mesa aqui,
# el dlopen de libGL.so.1 / libEGL.so.1 del motor no encuentra nada y el
# cliente muere con "Could not initialize OpenGL / GLES library". El motor
# trae SDL2 estatico y carga la capa grafica en runtime, asi que la ruta tiene
# que estar en el entorno (LD_LIBRARY_PATH) y no solo en el enlazador.
LIB_PATH="$MESA_LIB:$GLIBC_LIB"

if [ ! -f "$MESA_LIB/dri/zink_dri.so" ]; then
    log "ERROR: no esta el payload de Mesa en $MESA_DIR"
    log "       genera con: python3 scripts/prepare_mesa.py"
    exit 1
fi
if [ ! -f "$FACTORIO_DIR/bin/arm64/factorio" ]; then
    log "ERROR: no esta el cliente grafico en $FACTORIO_DIR/bin/arm64/factorio"
    exit 1
fi
if [ ! -f "$LOADER" ]; then
    log "ERROR: no esta el runtime de glibc en $GLIBC_DIR"
    exit 1
fi

# --- 2. Localizar el ICD de Vulkan de Android (el driver de Adreno) ---------
# Android guarda el ICD del fabricante en /vendor/lib64/hw (no en un
# subdirectorio "vulkan/", como en el escritorio). En la Pico 4 Ultra el
# driver es /vendor/lib64/hw/vulkan.adreno.so.
#
# PERO ese driver es un .so de BIONIC (depende de libc.so, libdl.so, liblog.so,
# libllvm-glnext.so y librerias HIDL). El cliente de Factorio es un binario
# GLIBC: cargar un driver bionic dentro de un proceso glibc mete dos libc en el
# mismo proceso y el driver revienta (o corrompe memoria sin avisar).
# Por eso aqui NO se usa el ICD del fabricante: se usa lavapipe (Vulkan por
# software, va con el payload de Mesa) para que el cliente tenga contexto GL.
# El render de verdad lo pone la app OpenXR, que si es nativa de Android.
VENDOR_VULKAN_DIR="${VENDOR_VULKAN_DIR:-/vendor/lib64/hw}"
ICD_JSON="$MESA_DIR/adreno_icd.json"
# Este ICD del fabricante es BIONIC y el cliente es un proceso GLIBC: si se
# carga, el motor revienta (dos libc en el mismo proceso).  Solo se usa si lo
# pide el llamante CONCRETAMENTE en ESTA ejecucion.
#
# OJO: antes se decidia por la mera EXISTENCIA del fichero JSON.  Ese JSON
# se queda en el disco (/data/local/tmp/mesa/adreno_icd.json), asi que una
# ejecucion anterior con XFACTORVR_USE_ADRENO_ICD=1 dejaba el fichero y la
# siguiente, sin la variable, se colgaba el ICD bionic sin querer.
USE_FABRICANTE=0

if [ "${XFACTORVR_USE_ADRENO_ICD:-0}" = "1" ]; then
    ICD_SO=""
    for cand in "$VENDOR_VULKAN_DIR"/vulkan.adreno.so \
                "$VENDOR_VULKAN_DIR"/vulkan/*.so \
                "$VENDOR_VULKAN_DIR"/libvulkan_adreno.so \
                "/system/lib64/hw/vulkan"/*.so; do
        if [ -f "$cand" ]; then
            ICD_SO="$cand"
            break
        fi
    done
    if [ -n "$ICD_SO" ]; then
        log "ICD de Vulkan del fabricante: $ICD_SO"
        cat > "$ICD_JSON" <<JSON
{
    "file_format_version": "1.0.0",
    "ICD": {
        "library_path": "$ICD_SO",
        "api_version": "1.3.239"
    }
}
JSON
        USE_FABRICANTE=1
    else
        log "AVISO: no se encontro un ICD de Vulkan en $VENDOR_VULKAN_DIR"
    fi
else
    log "ICD del fabricante ignorado (es bionic y el cliente es glibc)."
    log "  Se usa lavapipe (Vulkan por software) via Zink."
    log "  Para forzarlo: XFACTORVR_USE_ADRENO_ICD=1"
fi

# --- 3. Entorno de Mesa -----------------------------------------------------
# --- glvnd: donde encuentra el driver EGL de Mesa -------------------------
# libEGL.so.1 es el DESPACHADOR de glvnd, y el vendor real (libEGL_mesa.so.0)
# lo busca en /usr/share/glvnd/egl_vendor.d, que en el visor no existe.  Sin
# vendor, glvnd no expone ni extensiones ni devices, y SDL2 muere con
# "Could not find a valid EGL device to initialize".
# glvnd deja indicar donde esta con estas variables de entorno.
# El vendor de EGL se busca POR DIRECTORIO (glvnd lee los manifiestos JSON
# de ahi).  NO se pone __EGL_VENDOR_LIBRARY_FILENAMES: medido en el visor,
# con esa variable puesta glvnd NO llega a cargar el vendor (el trace del
# cargador muestra que nunca hace dlopen de libEGL_mesa.so.0), y el motor se
# queda sin devices EGL -> "Could not find a valid EGL device to initialize".
# Con solo __EGL_VENDOR_LIBRARY_DIRS: el vendor carga, aparece 1 device y el
# contexto sale sobre el Adreno (zink -> Turnip).
export __EGL_VENDOR_LIBRARY_DIRS="$MESA_DIR/glvnd_egl_vendor"
unset __EGL_VENDOR_LIBRARY_FILENAMES
mkdir -p "$__EGL_VENDOR_LIBRARY_DIRS"
cat > "$__EGL_VENDOR_LIBRARY_DIRS/10_mesa.json" <<JSON
{
    "file_format_version" : "1.0.0",
    "ICD" : {
        "library_path" : "$MESA_LIB/libEGL_mesa.so.0"
    }
}
JSON

export LIBGL_DRIVERS_PATH="$MESA_LIB/dri"
# El override del driver DRI (capa GL).  En Android no hay /dev/dri/renderD*
# y sin esto el DRI no autodetecta nada y no encuentra zink_dri.so.
# OJO: el loader de VULKAN no lee esta variable (ver scripts/spike_turnip_pico.sh);
# el ICD de Vulkan se elige aparte, con VK_ICD_FILENAMES.
# Se respeta el valor que venga puesto de fuera (lo usa el barrido del spike).
export MESA_LOADER_DRIVER_OVERRIDE="${MESA_LOADER_DRIVER_OVERRIDE:-zink}"
export ZINK_DESCRIPTORS=auto
export GALLIUM_DRIVER=zink
export LIBGL_ALWAYS_SOFTWARE=0
export LD_LIBRARY_PATH="$MESA_LIB:${LD_LIBRARY_PATH:-}"

# El ICD de Vulkan que se va a usar. Hay tres posibilidades y se eligen en este
# orden:
#   1. TURNIP  (driver abierto de Adreno de Mesa). Habla con el kernel por
#      /dev/kgsl-3d0 y NO es bionic, asi que se puede cargar dentro de un
#      proceso glibc sin meter dos libc en el mismo proceso. Es la via para
#      que el cliente grafico use la GPU de verdad en vez de lavapipe.
#      Necesita root (o permiso sobre /dev/kgsl-3d0).
#   2. El ICD del fabricante (/vendor/lib64/hw/vulkan.adreno.so): es bionic y
#      revienta dentro de un proceso glibc. Solo si lo pides a mano.
#   3. lavapipe (Vulkan por software): siempre funciona, pero sin GPU.
TURNIP_SO="$MESA_LIB/libvulkan_freedreno.so"
TURNIP_JSON="$MESA_DIR/turnip_icd.json"
LVP_SO="$MESA_LIB/libvulkan_lvp.so"
LVP_JSON="$MESA_DIR/lvp_icd.json"

# Si el llamante ya ha elegido un ICD (VK_ICD_FILENAMES puesto), se RESPETA:
# es lo que hace el barrido del spike para comparar turnip y lavapipe.
if [ -n "${VK_ICD_FILENAMES:-}" ]; then
    log "ICD elegido por el llamante: $VK_ICD_FILENAMES"
elif [ "${XFACTORVR_USE_TURNIP:-0}" = "1" ] && [ -f "$TURNIP_SO" ]; then
    cat > "$TURNIP_JSON" <<JSON
{
    "file_format_version": "1.0.0",
    "ICD": {
        "library_path": "$TURNIP_SO",
        "api_version": "1.1.230"
    }
}
JSON
    export VK_ICD_FILENAMES="$TURNIP_JSON"
    export VK_DRIVER_FILES="$TURNIP_JSON"
    log "turnip (Adreno por KGSL): $TURNIP_SO"
elif [ "$USE_FABRICANTE" = "1" ]; then
    export VK_ICD_FILENAMES="$ICD_JSON"
    export VK_DRIVER_FILES="$ICD_JSON"
    log "ICD del fabricante: $ICD_SO"
else
    # Sin ICD de Adreno: lavapipe (Vulkan por software) para que al menos arranque.
    # El JSON que trae el payload apunta a la ruta del host (/usr/lib/...), que
    # en el visor no existe: hay que rehacerlo con la ruta de aqui, o Mesa se
    # queda sin Vulkan y el cliente no puede crear el contexto OpenGL.
    if [ -f "$MESA_DIR/usr/share/vulkan/icd.d/lvp_icd.aarch64.json" ] && [ -f "$LVP_SO" ]; then
        cat > "$LVP_JSON" <<JSON
{
    "file_format_version": "1.0.0",
    "ICD": {
        "library_path": "$LVP_SO",
        "api_version": "1.3.239"
    }
}
JSON
        export VK_ICD_FILENAMES="$LVP_JSON"
        export VK_DRIVER_FILES="$LVP_JSON"
        log "lavapipe: $LVP_SO"
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
# El motor lleva curl enlazado estatico, con su propia resolucion de nombres
# (no encuentra /etc/resolv.conf y usa 127.0.0.1, donde no hay nada).  El
# shim trae un proxy CONNECT y un resolutor DNS propios: con el proxy, el
# padlock del auth server sale de verdad y el cliente se puede emparejar.
PRELOAD=""
# OJO: el CLIENTE **no** necesita el resolutor de nombres.
#
# Medido en el visor Pico: con el shim preloaded el motor se queda ESPERADO
# despues de "Factorio initialised" y nunca llega a "Joining game" (el hilo
# principal acaba en hrtimer_nanosleep).  Sin el shim, el motor entra en
# juego del tiron ("InGame" + "PlayerJoinGame") contra el servidor LAN.
#
# El shim solo lo necesita el SERVIDOR, que sin el no puede pedir el
# "server padlock" a auth.factorio.com y se queda sin partida.
#
# Por defecto el cliente NO lo carga.  Se puede forzar con
# XFACTORVR_RESOLVER=/ruta/libxfactorvr_resolver.so
RESOLVER="${XFACTORVR_RESOLVER:-}"
PROXY_PUERTO="${XFACTORVR_PROXY_PUERTO:-8899}"
if [ -f "$RESOLVER" ]; then
    export https_proxy="http://127.0.0.1:$PROXY_PUERTO"
    export http_proxy="http://127.0.0.1:$PROXY_PUERTO"
    export XFACTORVR_RESOLVER_LOG="${XFACTORVR_RESOLVER_LOG:-$CLIENT_DIR/logs/resolver.log}"
    mkdir -p "$(dirname "$XFACTORVR_RESOLVER_LOG")" 2>/dev/null || true
    PRELOAD="$RESOLVER${PRELOAD:+:$PRELOAD}"
    log "resolutor: $RESOLVER (proxy en 127.0.0.1:$PROXY_PUERTO)"
fi

INJECTOR="${INJECTOR:-/data/local/tmp/libxfactorvr_injector.so}"
if [ -f "$INJECTOR" ] && [ "${XFACTORVR_USE_INJECTOR:-1}" = "1" ]; then
    PRELOAD="$INJECTOR${PRELOAD:+:$PRELOAD}"
    log "inyector: $INJECTOR"
    # Destino de los hooks: por defecto el render del juego.
    export XFACTORVR_HOOK_TARGET="${XFACTORVR_HOOK_TARGET:-GameRenderer::render}"
    export XFACTORVR_HOOK_EVERY="${XFACTORVR_HOOK_EVERY:-60}"
    # ESTEREO REAL: el motor dibuja el mundo desplazado a la izquierda en un
    # frame y a la derecha en el siguiente, y el visor muestra cada vista en
    # su ojo.  El desplazamiento va en pixeles del framebuffer (24 px es el
    # paralaje de una IPD de 64 mm a un metro de distancia).  Con 0 se
    # desactiva (el visor entonces desplaza la UV: efecto "TV 3D").
    export XFACTORVR_3D_CAMARA="${XFACTORVR_3D_CAMARA:-24}"
    # Envio del render real al visor VR: el APK escucha en 127.0.0.1:27020
    # y el juego le manda el framebuffer del mundo (protocolo XFV1).
    # Se puede desactivar con XFACTORVR_STREAM=0.
    if [ "${XFACTORVR_STREAM:-1}" = "1" ]; then
        export XFACTORVR_STREAM="${XFACTORVR_STREAM_ADDR:-127.0.0.1:27020}"
        export XFACTORVR_STREAM_ANCHO="${XFACTORVR_STREAM_ANCHO:-640}"
        export XFACTORVR_STREAM_ALTO="${XFACTORVR_STREAM_ALTO:-360}"
        export XFACTORVR_STREAM_FPS="${XFACTORVR_STREAM_FPS:-12}"
        log "render real al visor: $XFACTORVR_STREAM (${XFACTORVR_STREAM_ANCHO}x${XFACTORVR_STREAM_ALTO})"
    fi
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

# --- 5. Configuracion propia del cliente ------------------------------------
# El motor deduce sus rutas de donde esta el config-path.cfg. Para que el
# servidor y el cliente compartan el MISMO arbol de datos (1,4 GB) pero tengan
# cada uno su write-data (y su .lock), el cliente usa un config.ini propio:
#   [path]
#   read-data=<raiz del juego>/data
#   write-data=<directorio del cliente>
# Es la unica forma de que el motor lo acepte: si el ejecutable esta en otro
# sitio, deduce el write-data por si mismo y choca con el del servidor.
GAME_DIR="${GAME_DIR_OVERRIDE:-${GAME_DIR:-/data/local/tmp/factorio}}"
SERVER_PORT="${SERVER_PORT:-34197}"

mkdir -p "$CLIENT_DIR/config" "$CLIENT_DIR/saves" "$CLIENT_DIR/logs" 2>/dev/null || true
CLIENT_CONFIG="$CLIENT_DIR/config/config.ini"
if [ ! -f "$CLIENT_CONFIG" ]; then
    log "Creando el config.ini del cliente..."
    cat > "$CLIENT_CONFIG" <<CFG
[path]
read-data=$GAME_DIR/data
write-data=$CLIENT_DIR

[other]
check-updates=false
CFG
fi

# El motor abre un dialogo MODAL ("Audio error: Couldn't initialize audio:
# Audio device open failed") cuando no puede abrir el dispositivo de audio.
# Ese dialogo se come el teclado: ni la consola del juego ni el raton
# funcionan.  En el visor el sonido lo pone el motor del APK, asi que el
# cliente va sin audio (ademas el paquete ligero no trae audio).
if ! grep -q "^sound-enabled" "$CLIENT_CONFIG" 2>/dev/null; then
    log "Desactivando el audio del cliente (evita el dialogo modal de error)..."
    printf '\n[sound]\nsound-enabled=false\n' >> "$CLIENT_CONFIG"
fi

# --- 5a. Mods: la MISMA lista que el servidor ------------------------------
#
# El servidor y el cliente deben llevar exactamente la misma lista de mods o
# la conexion se rechaza.  Ademas el cliente GRAFICO comprueba la licencia de
# las expansiones y, sin cuenta de Factorio, se niega a cargarlas:
#   "Failed to load mods: Mod elevated-rails requires Space Age expansion."
# Por eso:
#   1. se copia al cliente la lista canonica (base + mod puente),
#   2. se copia el mod puente desde el arbol del servidor (para que ambos
#      lados ejecuten EXACTAMENTE el mismo control.lua),
#   3. se apartan las expansiones del read-data del cliente (el motor las
#      auto-activa si las encuentra y luego no las deja cargar).
MOD_LIST_CANON="${MOD_LIST_CANON:-/data/local/tmp/mod-list.json}"
SERVER_MOD_DIR="${SERVER_MOD_DIR:-/data/local/tmp/factorio_arm64/mods}"
CLIENT_MODS="$CLIENT_DIR/mods"
mkdir -p "$CLIENT_MODS" 2>/dev/null || true

if [ ! -f "$MOD_LIST_CANON" ]; then
    cat > "$MOD_LIST_CANON" <<'JSON'
{
  "mods":
  [
    { "name": "base", "enabled": true },
    { "name": "xfactorvr_bridge", "enabled": true }
  ]
}
JSON
fi
cp "$MOD_LIST_CANON" "$CLIENT_MODS/mod-list.json" 2>/dev/null && \
    log "Lista de mods del cliente: $CLIENT_MODS/mod-list.json"

# El mod puente, copiado del servidor para que no haya ni un byte de
# diferencia entre los dos lados.
for _d in "$SERVER_MOD_DIR"/xfactorvr_bridge_*/; do
    [ -f "${_d}control.lua" ] || continue
    _nombre=$(basename "$_d")
    if [ ! -f "$CLIENT_MODS/$_nombre/control.lua" ] || \
       ! cmp -s "$_d/control.lua" "$CLIENT_MODS/$_nombre/control.lua"; then
        mkdir -p "$CLIENT_MODS/$_nombre" 2>/dev/null || true
        cp "$_d"control.lua "$_d"info.json "$CLIENT_MODS/$_nombre/" 2>/dev/null
        log "Mod puente copiado del servidor: $_nombre"
    fi
done

# Las expansiones NO pueden estar en el read-data del cliente grafico.
# El read-data se lee del config.ini (que es el que manda de verdad: el
# motor deduce sus rutas de ahi), y solo si no se encuentra se usa el
# arbol del cliente.
READ_DATA_DIR="$(sed -n 's/^read-data=//p' "$CLIENT_CONFIG" 2>/dev/null | head -1)"
[ -n "$READ_DATA_DIR" ] || READ_DATA_DIR="$GAME_DIR/data"
for _exp in space-age quality recycler elevated-rails; do
    if [ -d "$READ_DATA_DIR/$_exp" ]; then
        mkdir -p "$READ_DATA_DIR/_sin_licencia" 2>/dev/null || true
        mv "$READ_DATA_DIR/$_exp" "$READ_DATA_DIR/_sin_licencia/" 2>/dev/null && \
            log "Expansion apartada (el cliente grafico no tiene licencia): $_exp"
    fi
done

# --- 5b. La consola del motor (canal de ordenes sin RCON, ADR-020) ---------
#
# El motor lee ordenes por su stdin (la consola de Factorio).  Si el stdin
# esta a EOF, el motor CIERRA el lector y ya no acepta ni una orden mas
# ("Got EOF on stdin; closing").  Con una FIFO abierta en lectura-escritura
# nunca hay EOF, y el inyector (que vive dentro del motor) puede escribir en
# el fd 0 del propio motor.
CONSOLA_FIFO="${CONSOLA_FIFO:-$CLIENT_DIR/consola.fifo}"
CONSOLA_DIR="${XFACTORVR_CONSOLA_DIR:-/sdcard/XFactorVR/consola}"
mkdir -p "$(dirname "$CONSOLA_FIFO")" "$CONSOLA_DIR" 2>/dev/null || true
if [ ! -p "$CONSOLA_FIFO" ]; then
    mkfifo "$CONSOLA_FIFO" 2>/dev/null || true
fi
STDIN_MOTOR=""
if [ -p "$CONSOLA_FIFO" ] && exec 3<>"$CONSOLA_FIFO" 2>/dev/null; then
    # Se abre en lectura-escritura: el motor lee del fd 0 y no hay EOF.
    STDIN_MOTOR="<&3"
    export XFACTORVR_CONSOLA_DIR="$CONSOLA_DIR"
    log "Consola del motor: FIFO $CONSOLA_FIFO, ordenes en $CONSOLA_DIR"
else
    # Sin FIFO el motor leeria de /dev/null: cerraria la consola al instante
    # y no aceptaria ni una orden.  Se le pasa /dev/null igualmente, pero sin
    # romper el arranque (solo se pierde el canal de ordenes).
    log "AVISO: no se pudo abrir la FIFO de la consola ($CONSOLA_FIFO):"
    log "       el motor no aceptara ordenes por consola (solo RCON)."
fi

# --- 6. Arranque ------------------------------------------------------------
GAME_BIN="$GAME_DIR/bin/arm64/factorio"

# Si quedo un cliente anterior vivo, el motor nuevo muere al instante:
#   "Couldn't acquire exclusive lock for /data/local/tmp/factorio_cliente/.lock"
# y, sin ventana, solo se ve el dialogo de error.  Se mata SOLO el cliente
# anterior (el del servidor se reconoce por su --config, no por el nombre:
# en el visor el proceso se llama "ld-linux-aarch64.so.1").
for _p in /proc/[0-9]*; do
    [ -r "$_p/cmdline" ] || continue
    [ "$_p" = "/proc/$$" ] && continue
    _cmd=$(tr '\0' ' ' < "$_p/cmdline" 2>/dev/null)
    case "$_cmd" in
        *bin/arm64/factorio*"--config $CLIENT_CONFIG"*)
            log "Matando el cliente anterior (pid ${_p#/proc/})..."
            kill "${_p#/proc/}" 2>/dev/null
            ;;
    esac
done
sleep 1

if [ -n "${SERVER_ADDR:-}" ]; then
    # Modo campana: el juego corre en dos procesos del mismo arbol. El
    # SERVIDOR simula y tiene el RCON (el RCON es funcion del servidor) y el
    # CLIENTE se conecta para aportar el jugador que el nivel de campana
    # exige (usa game.players[1]) y para pintar el mundo en 3D.
    log "Conectando al servidor $SERVER_ADDR:$SERVER_PORT..."
    exec env LD_LIBRARY_PATH="$LIB_PATH" LD_PRELOAD="$PRELOAD" "$LOADER" --library-path "$LIB_PATH" "$GAME_BIN" \
        --config "$CLIENT_CONFIG" \
        --window-size "${WIDTH}x${HEIGHT}" \
        --disable-audio \
        --mp-connect "$SERVER_ADDR:$SERVER_PORT" $STDIN_MOTOR
fi

# Modo simple: el cliente juega en single-player (sin RCON, que en single
# player no existe) sobre una partida guardada.  La partida va en el
# write-data DEL CLIENTE (el del servidor es de root y no se puede escribir).
SAVE="${SAVE:-$CLIENT_DIR/saves/xfactorvr.zip}"
if [ ! -f "$SAVE" ]; then
    log "Creando la partida inicial..."
    env LD_LIBRARY_PATH="$LIB_PATH" LD_PRELOAD="$PRELOAD" "$LOADER" --library-path "$LIB_PATH" "$GAME_BIN" --create "$SAVE" --config "$CLIENT_CONFIG" \
        2>&1 | tail -20
fi

log "Arrancando el cliente grafico (Zink -> Vulkan) en $WIDTH x $HEIGHT..."
log "  ICD: ${VK_ICD_FILENAMES:-ninguno}"
log "  SDL: $SDL_VIDEODRIVER"

exec env LD_LIBRARY_PATH="$LIB_PATH" LD_PRELOAD="$PRELOAD" "$LOADER" --library-path "$LIB_PATH" "$GAME_BIN" \
    --config "$CLIENT_CONFIG" \
    --window-size "${WIDTH}x${HEIGHT}" \
    --fullscreen \
    "$SAVE" $STDIN_MOTOR
