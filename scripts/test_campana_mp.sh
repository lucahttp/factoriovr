#!/bin/bash
# test_campana_mp.sh — Banco de la CAMPAÑA en headless + cliente gráfico MP.
#
# Comprueba, de verdad, que la campaña se puede jugar con esta arquitectura:
#
#     servidor headless (simulación + RCON + mod puente)
#          +  cliente gráfico (Mesa/Zink) que se conecta y crea el JUGADOR
#
# Las comprobaciones (todas tienen que pasar para salir con 0):
#   1. El servidor arranca con el escenario de campaña y abre el RCON.
#   2. El cliente se conecta de verdad: se comprueba por RCON que
#      #game.connected_players > 0 y que el nivel tiene jugador.
#   3. El escenario NO ha petado: 0 "non-recoverable error" en el log del
#      servidor (ni en el del cliente).
#   4. El tick del servidor AVANZA entre dos consultas RCON separadas por
#      unos segundos (con auto_pause el mapa se congela si no hay jugador,
#      así que esto demuestra que la simulación corre de verdad).
#   5. El mod puente (xfactorvr_bridge) responde por RCON con el estado del
#      mundo: la instantánea trae entidades y terreno.
#   6. El cliente usa SU PROPIO árbol de datos (write-data distinto del del
#      servidor) y comparte los datos de solo lectura del juego: es lo que
#      evita pelear por el .lock y por la config.
#
# Códigos de salida:
#   0  todo correcto
#   1  algo falla
#   2  el entorno no permite validarlo (sin podman, sin contenedor, sin
#      binario, sin datos, sin X, ...)
#
# Uso: bash scripts/test_campana_mp.sh
#
# Variables de entorno:
#   PODMAN          binario de podman (por defecto: podman)
#   CONTAINER       contenedor (por defecto: xfactorvr-client)
#   FACTIO_DIR      raíz del servidor dentro del contenedor (/factorio)
#   CLIENT_DIR      raíz del cliente dentro del contenedor (/factorio_cliente)
#   ESCENARIO       escenario de campaña (base/xfv-nivel)
#   PUERTO          puerto del juego (34197)
#   RCON_PORT       puerto del RCON (27015)
#   RCON_PASSWORD   contraseña del RCON (xfactorvr)
#   DISPLAY_NUM     display de Xvfb a usar o crear (:99)
#   XFV_TIMEOUT     segundos de espera maxima por fase (180)
#   XFV_ESPERA_JUGADOR  segundos que se deja correr la historia tras la
#                      conexión (20) antes de comprobar nada

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-podman}"
CONTAINER="${CONTAINER:-xfactorvr-client}"
FACTORIO_DIR="${FACTORIO_DIR:-/factorio}"
CLIENT_DIR="${CLIENT_DIR:-/factorio_cliente}"
ESCENARIO="${ESCENARIO:-base/xfv-nivel}"
PUERTO="${PUERTO:-34197}"
RCON_PORT="${RCON_PORT:-27015}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
DISPLAY_NUM="${DISPLAY_NUM:-99}"
XFV_TIMEOUT="${XFV_TIMEOUT:-180}"
XFV_ESPERA_JUGADOR="${XFV_ESPERA_JUGADOR:-20}"

PROBE="$PROJECT_ROOT/scripts/rcon_probe.py"
PARCHEADOR="$PROJECT_ROOT/scripts/preparar_campana_mp.py"
MOD_PUENTE="$PROJECT_ROOT/mods/xfactorvr_bridge"

# Ficheros dentro del contenedor.
SERVER_LOG="$FACTORIO_DIR/campana_mp_server.log"
CLIENT_STDOUT="$FACTORIO_DIR/campana_mp_client.log"
CLIENT_LOG="$CLIENT_DIR/factorio-current.log"
SERVER_SETTINGS="$FACTORIO_DIR/config/server-settings.json"
CLIENT_CONFIG="$CLIENT_DIR/config/config.ini"
CMDS_A="$FACTORIO_DIR/tmp/campana_mp_cmds_a.txt"
CMDS_B="$FACTORIO_DIR/tmp/campana_mp_cmds_b.txt"

FALLOS=0
PASOS=0

log()   { printf '%s\n' "$*"; }
paso()  { PASOS=$((PASOS + 1)); printf '\n=== [%d] %s ===\n' "$PASOS" "$*"; }
ok()    { printf '  OK    %s\n' "$*"; }
fail()  { FALLOS=$((FALLOS + 1)); printf '  FALLO %s\n' "$*"; }
aviso() { printf '  AVISO %s\n' "$*"; }
sin_entorno() {
    printf '\nENTORNO NO VALIDABLE: %s\n' "$*" >&2
    printf 'codigo de salida 2\n'
    exit 2
}

in_container() { "$PODMAN" exec "$CONTAINER" bash -c "$1"; }

contiene() {  # $1 = patron grep, $2 = fichero
    in_container "grep -c '$1' '$2' 2>/dev/null || true" | tr -d '[:space:]'
}

# --------------------------------------------------------------------------
# 0. Preflight del entorno
# --------------------------------------------------------------------------
paso "Preflight del entorno"

command -v "$PODMAN" >/dev/null 2>&1 || sin_entorno "no esta '$PODMAN' en el PATH"
"$PODMAN" ps --format '{{.Names}}' 2>/dev/null | grep -qx "$CONTAINER" \
    || sin_entorno "el contenedor '$CONTAINER' no esta arrancado"

[ -f "$PROBE" ] || sin_entorno "falta scripts/rcon_probe.py"
[ -f "$PARCHEADOR" ] || sin_entorno "falta scripts/preparar_campana_mp.py"
[ -d "$MOD_PUENTE" ] || sin_entorno "falta mods/xfactorvr_bridge"

for ruta in "$FACTORIO_DIR/bin/arm64/factorio" "$FACTORIO_DIR/data/base" \
            "$FACTORIO_DIR/data/core" "$CLIENT_DIR"; do
    in_container "test -e '$ruta'" || sin_entorno "en el contenedor no esta '$ruta'"
done

ESC_DIR="$FACTORIO_DIR/data/$ESCENARIO"
case "$ESCENARIO" in
    */*)  # "MOD/NAME" -> data/MOD/scenarios/NAME
        ESC_DIR="$FACTORIO_DIR/data/${ESCENARIO%%/*}/scenarios/${ESCENARIO#*/}" ;;
    *)    # "NAME"    -> data/scenarios/NAME
        ESC_DIR="$FACTORIO_DIR/data/scenarios/$ESCENARIO" ;;
esac
in_container "test -f '$ESC_DIR/control.lua'" \
    || sin_entorno "el escenario '$ESCENARIO' no esta en $ESC_DIR"
in_container "test -f '$ESC_DIR/blueprint.zip'" \
    || sin_entorno "al escenario '$ESCENARIO' le falta blueprint.zip"

in_container "command -v python3 >/dev/null" || sin_entorno "el contenedor no tiene python3"

# Driver de video del cliente.
#
# OJO: el SDL que lleva el motor ENLAZADO ESTATICAMENTE NO tiene X11.
# Medido en el binario real (factorio/bin/arm64/factorio), los drivers que
# trae son: dummy, kmsdrm, offscreen y wayland.  No hay cadena "x11" en el
# binario, asi que forzar SDL_VIDEODRIVER=x11 hace que SDL_Init falle con
#   "SDL couldn't be initialized. SDL_Error: x11 not available"
# y el cliente muera antes de conectar.  Por defecto se usa "offscreen",
# que es el mismo driver que usa el cliente en el visor Pico (allí tampoco
# hay X11).
SDL_VIDEO="${SDL_VIDEO:-offscreen}"
case "$SDL_VIDEO" in
    x11)
        in_container "command -v Xvfb >/dev/null" \
            || sin_entorno "se pidio x11 pero el contenedor no tiene Xvfb"
        in_container "Xvfb :$DISPLAY_NUM -screen 0 1280x720x24 -nolisten tcp >/dev/null 2>&1 & sleep 3" \
            || sin_entorno "no se pudo arrancar Xvfb"
        ;;
esac
ok "contenedor '$CONTAINER' con datos del juego y driver de video $SDL_VIDEO"

# --------------------------------------------------------------------------
# 1. Preparacion: nivel parcheado para MP, mod puente, config y ajustes
# --------------------------------------------------------------------------
paso "Preparacion (nivel para MP, mod puente, arbol de datos del cliente)"

"$PODMAN" cp "$PARCHEADOR" "$CONTAINER:/tmp/preparar_campana_mp.py" >/dev/null
in_container "python3 /tmp/preparar_campana_mp.py '$ESC_DIR'" | sed 's/^/  /'

# El mod puente tiene que estar en los DOS lados con la misma lista de mods
# (en multijugador cliente y servidor deben coincidir).
"$PODMAN" cp "$MOD_PUENTE" "$CONTAINER:$FACTORIO_DIR/mods/" >/dev/null
"$PODMAN" cp "$MOD_PUENTE" "$CONTAINER:$CLIENT_DIR/mods/" >/dev/null
for d in "$FACTORIO_DIR/mods" "$CLIENT_DIR/mods"; do
    in_container "cat > '$d/mod-list.json' <<'JSON'
{
  \"mods\": [
    { \"name\": \"base\", \"enabled\": true },
    { \"name\": \"xfactorvr_bridge\", \"enabled\": true }
  ]
}
JSON"
done
ok "mod puente instalado en servidor y cliente"

# Ajustes del servidor: sin verificacion de usuario (el cliente del banco no
# tiene cuenta de factorio.com) y auto_pause ENCENDIDO (mientras no haya
# jugador el mapa se congela y el nivel de campana no puede petar).
in_container "mkdir -p '$FACTORIO_DIR/config' && cat > '$SERVER_SETTINGS' <<'JSON'
{
  \"name\": \"XFactorVR-Campana\",
  \"description\": \"Campana XFactorVR (servidor headless + cliente grafico)\",
  \"visibility\": { \"public\": false, \"lan\": true },
  \"require_user_verification\": false,
  \"max_players\": 4,
  \"autosave_interval\": 10,
  \"autosave_slots\": 3,
  \"auto_pause\": true,
  \"auto_pause_when_players_connect\": false,
  \"only_admins_can_pause_the_game\": true,
  \"allow_commands\": \"admins-only\"
}
JSON"
ok "ajustes de servidor escritos (require_user_verification=false, auto_pause=true)"

# Arbol de datos PROPIO del cliente: mismo 'data' de solo lectura (el del
# juego) y otro 'write-data' (config, saves, mods y el lock). Asi el cliente
# no pelea por el .lock del servidor y no hay que copiar 261 MB.
in_container "mkdir -p '$CLIENT_DIR/config' '$CLIENT_DIR/mods' '$CLIENT_DIR/saves' '$CLIENT_DIR/script-output' && cat > '$CLIENT_CONFIG' <<'CFG'
[path]
read-data=$FACTORIO_DIR/data
write-data=$CLIENT_DIR

[general]
locale=en

[other]
check-updates=false
CFG"
ok "config del cliente: read-data=$FACTORIO_DIR/data, write-data=$CLIENT_DIR"

# --------------------------------------------------------------------------
# 2. Limpieza de procesos anteriores y arranque del servidor
# --------------------------------------------------------------------------
paso "Arranque del servidor headless con el escenario de campana"

in_container "pkill -f 'arm64/factor''io' >/dev/null 2>&1; sleep 2; true"

in_container "cd '$FACTORIO_DIR' && (sleep 1800 | ./bin/arm64/factorio \
    --start-server-load-scenario '$ESCENARIO' \
    --port $PUERTO --rcon-port $RCON_PORT --rcon-password '$RCON_PASSWORD' \
    --server-settings '$SERVER_SETTINGS') > '$SERVER_LOG' 2>&1 &"

RCON_LISTO=0
for i in $(seq 1 60); do
    if in_container "grep -q 'Starting RCON interface' '$SERVER_LOG' 2>/dev/null"; then
        RCON_LISTO=1
        break
    fi
    sleep 1
done
if [ "$RCON_LISTO" = "1" ]; then
    ok "servidor arrancado y RCON escuchando en el puerto $RCON_PORT (tras ${i}s)"
else
    fail "el servidor no abrio el RCON"
    in_container "tail -25 '$SERVER_LOG'" | sed 's/^/  /'
fi

if in_container "grep -q 'non-recoverable' '$SERVER_LOG' 2>/dev/null"; then
    fail "el escenario ya peta antes de que entre el cliente"
    in_container "grep -A 12 'non-recoverable' '$SERVER_LOG' | head -20" | sed 's/^/  /'
else
    ok "el escenario no peta con el servidor solo (auto_pause lo congela)"
fi

# --------------------------------------------------------------------------
# 3. Arranque del cliente grafico y conexion
# --------------------------------------------------------------------------
paso "Arranque del cliente grafico y conexion al servidor"

# El inyector es la pieza que engancha el render del juego para el visor 3D.
# En el banco se usa si esta disponible (es la cadena real del visor), pero
# no es imprescindible para jugar la campana.
PRELOAD_INYECTOR=""
if [ "${XFV_USAR_INYECTOR:-auto}" != "0" ] && in_container "test -f /injector.so"; then
    PRELOAD_INYECTOR="LD_PRELOAD=/injector.so XFACTORVR_HOOK_EVERY=60"
    ok "el cliente arranca con el inyector (/injector.so)"
else
    aviso "el cliente arranca SIN inyector (no hay /injector.so o esta desactivado)"
fi

in_container "cd '$CLIENT_DIR' && env SDL_VIDEODRIVER=$SDL_VIDEO \
    XDG_RUNTIME_DIR=/tmp/xdg \
    LIBGL_DRIVERS_PATH=/usr/lib/aarch64-linux-gnu/dri \
    MESA_LOADER_DRIVER_OVERRIDE=zink ZINK_DESCRIPTORS=auto \
    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
    LIBGL_ALWAYS_SOFTWARE=1 $PRELOAD_INYECTOR \
    '$FACTORIO_DIR/bin/arm64/factorio' \
    --config '$CLIENT_CONFIG' \
    --mp-connect 127.0.0.1:$PUERTO \
    --window-size 640x360 --disable-audio > '$CLIENT_STDOUT' 2>&1 &"

CONECTADO=0
for i in $(seq 1 "$XFV_TIMEOUT"); do
    if in_container "grep -q 'processed PlayerJoinGame' '$SERVER_LOG' 2>/dev/null"; then
        CONECTADO=1
        break
    fi
    if in_container "grep -q 'non-recoverable' '$SERVER_LOG' 2>/dev/null"; then
        break
    fi
    sleep 1
done
if [ "$CONECTADO" = "1" ]; then
    ok "el cliente se conecto y el servidor creo el jugador (tras ${i}s)"
    in_container "grep -E 'PlayerJoinGame|joined the game' '$SERVER_LOG' | tail -3" | sed 's/^/  /'
else
    fail "el cliente no se conecto"
    in_container "tail -20 '$SERVER_LOG'" | sed 's/^/  /'
    in_container "tail -20 '$CLIENT_STDOUT'" | sed 's/^/  /'
fi

if [ "$CONECTADO" = "1" ]; then
    printf '  ... dejando correr la historia del nivel %ss\n' "$XFV_ESPERA_JUGADOR"
    sleep "$XFV_ESPERA_JUGADOR"
fi

# --------------------------------------------------------------------------
# 4. Comprobaciones por RCON
# --------------------------------------------------------------------------
paso "Comprobaciones por RCON (jugador, tick y mod puente)"

# rcon_query <fichero de comandos dentro del contenedor> -> imprime
# "orden<TAB>cuerpo" con el cuerpo de la respuesta de cada orden.
PARSER_RCON='
import ast, re, sys
orden = None
for linea in sys.stdin.read().splitlines():
    if linea.startswith("$ "):
        orden = linea[2:].strip()
        continue
    if orden is None:
        continue
    m = re.match(r"\s*respuesta \(\d+ paquete/s\): (.*)$", linea)
    if m:
        bruto = m.group(1).strip()
        try:
            cuerpo = ast.literal_eval(bruto)
        except Exception:
            cuerpo = bruto
        cuerpo = cuerpo.replace("\n", " ").replace("\t", " ").strip()
        print("%s\t%s" % (orden, cuerpo))
        orden = None
'

rcon_query() {
    "$PODMAN" exec -i "$CONTAINER" python3 - --file "$1" "$RCON_PORT" "$RCON_PASSWORD" \
        < "$PROBE" 2>/dev/null | python3 -c "$PARSER_RCON"
}

valor_de() {  # $1 = prefijo de la orden, $2 = texto de la salida
    printf '%s\n' "$2" | awk -F'\t' -v p="$1" '
        index($1, p) == 1 { if ($2 != "" && !v) { v = $2; } } END { print v }'
}

in_container "mkdir -p '$FACTORIO_DIR/tmp'"

# El primer comando de consola de cada sesion RCON puede ser rechazado por
# el motor, asi que cada orden va duplicada: se usa la primera respuesta no
# vacia.
in_container "cat > '$CMDS_A' <<'CMDS'
/silent-command rcon.print('tick1='..game.tick)
/silent-command rcon.print('tick1='..game.tick)
/silent-command rcon.print('conectados='..#game.connected_players)
/silent-command rcon.print('conectados='..#game.connected_players)
/silent-command rcon.print('jugadores='..#game.players)
/silent-command rcon.print('jugadores='..#game.players)
/silent-command rcon.print('nombre='..tostring(game.players[1] and game.players[1].name))
/silent-command rcon.print('nombre='..tostring(game.players[1] and game.players[1].name))
/silent-command rcon.print('pos='..tostring(game.players[1] and game.players[1].position and game.players[1].position.x)..','..tostring(game.players[1] and game.players[1].position and game.players[1].position.y))
/silent-command rcon.print('pos='..tostring(game.players[1] and game.players[1].position and game.players[1].position.x)..','..tostring(game.players[1] and game.players[1].position and game.players[1].position.y))
/silent-command remote.call('xfactorvr', 'get_world_snapshot', 16, 0, 0)
/silent-command remote.call('xfactorvr', 'get_world_snapshot', 16, 0, 0)
/silent-command remote.call('xfactorvr', 'teleport_player', 0, 0)
/silent-command remote.call('xfactorvr', 'teleport_player', 0, 0)
/silent-command remote.call('xfactorvr', 'interact_entity', 0, 0)
/silent-command remote.call('xfactorvr', 'interact_entity', 0, 0)
CMDS"

in_container "cat > '$CMDS_B' <<'CMDS'
/silent-command rcon.print('tick2='..game.tick)
/silent-command rcon.print('tick2='..game.tick)
CMDS"

SALIDA_A="$(rcon_query "$CMDS_A")"
TICK1="$(valor_de "/silent-command rcon.print('tick1" "$SALIDA_A")"
CONECTADOS="$(valor_de "/silent-command rcon.print('conectados" "$SALIDA_A")"
JUGADORES="$(valor_de "/silent-command rcon.print('jugadores" "$SALIDA_A")"
NOMBRE="$(valor_de "/silent-command rcon.print('nombre" "$SALIDA_A")"
SNAPSHOT="$(valor_de "/silent-command remote.call('xfactorvr'" "$SALIDA_A")"

printf '  RCON: %s | %s | %s | %s\n' "$TICK1" "$CONECTADOS" "$JUGADORES" "$NOMBRE"
printf '  RCON: snapshot del puente: %.140s...\n' "$SNAPSHOT"

# 4a. el jugador esta conectado
case "$CONECTADOS" in
    conectados=*)
        n="${CONECTADOS#conectados=}"; n="${n%% *}"
        if [ "${n:-0}" -gt 0 ] 2>/dev/null; then
            ok "el RCON ve $CONECTADOS (jugador conectado)"
        else
            fail "el RCON no ve ningun jugador conectado ($CONECTADOS)"
        fi ;;
    *) fail "el RCON no devolvio el numero de jugadores conectados ($CONECTADOS)" ;;
esac

case "$JUGADORES" in
    jugadores=*)
        n="${JUGADORES#jugadores=}"; n="${n%% *}"
        if [ "${n:-0}" -ge 1 ] 2>/dev/null; then
            ok "el nivel tiene $JUGADORES (el escenario crea al jugador)"
        else
            fail "el nivel no tiene jugador ($JUGADORES)"
        fi ;;
    *) fail "el RCON no devolvio el numero de jugadores del nivel ($JUGADORES)" ;;
esac

# 4b. el tick avanza entre dos consultas separadas por unos segundos
sleep 5
SALIDA_B="$(rcon_query "$CMDS_B")"
TICK2="$(valor_de "/silent-command rcon.print('tick2" "$SALIDA_B")"
printf '  RCON: %s -> %s\n' "$TICK1" "$TICK2"

n1="${TICK1#tick1=}"; n1="${n1%% *}"
n2="${TICK2#tick2=}"; n2="${n2%% *}"
if [ -n "$n1" ] && [ -n "$n2" ] && [ "$n2" -gt "$n1" ] 2>/dev/null; then
    ok "el tick avanza ($n1 -> $n2): la simulacion corre"
else
    fail "el tick NO avanza ($TICK1 -> $TICK2)"
fi

# 4c. el mod puente responde con el estado del mundo
case "$SNAPSHOT" in
    *'"entities"'*)
        n_ent="$(printf '%s' "$SNAPSHOT" | grep -o '"t":"' | wc -l | tr -d ' ')"
        ok "el mod puente responde por RCON con el estado del mundo ($n_ent entidades)"
        ;;
    *) fail "el mod puente no devolvio la instantanea del mundo" ;;
esac

# 4e. el canal de estado POR FICHERO lleva la posicion del jugador.
# Esto es lo que coloca la mesa del visor: sin esto el visor se queda en
# (0,0) y pinta un trozo de mapa que no es donde esta la fabrica.
ESTADO_IN="$FACTORIO_DIR/script-output/xfactorvr_bridge/estado.json"
if in_container "test -f '$ESTADO_IN'"; then
    ESTADO="$(in_container "cat '$ESTADO_IN'" | tr -d '\r' | tail -c 200000)"
    POS_MOD="$(printf '%s' "$ESTADO" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception:
    print(''); raise SystemExit
j = d.get('jugador') or {}
print('%s,%s' % (j.get('x'), j.get('y')))
" 2>/dev/null)"
    POS_REAL="$(valor_de "/silent-command rcon.print('pos" "$SALIDA_A")"
    printf '  posicion: RCON=%s  estado.json=%s\n' "${POS_REAL:-?}" "${POS_MOD:-?}"
    if [ -n "$POS_MOD" ] && [ -n "$POS_REAL" ]; then
        if printf '%s\n%s\n' "$POS_REAL" "$POS_MOD" | python3 -c "
import sys
a = sys.stdin.read().split()
try:
    rx, ry = [float(v) for v in a[0].split(',')]
    mx, my = [float(v) for v in a[1].split(',')]
except Exception:
    sys.exit(1)
sys.exit(0 if abs(rx - mx) <= 2.0 and abs(ry - my) <= 2.0 else 1)
"; then
            ok "el estado por fichero lleva la posicion del jugador (la camara del visor)"
        else
            fail "el estado por fichero NO lleva la posicion real del jugador"
        fi
    else
        fail "no se pudo leer la posicion del jugador ($POS_REAL / $POS_MOD)"
    fi
else
    fail "el mod no escribio el estado por fichero ($ESTADO_IN)"
fi

# 4d. el jugador se puede mover e interactuar (lo que pide el final del nivel)
TELEPORT="$(valor_de "/silent-command remote.call('xfactorvr', 'teleport_player'" "$SALIDA_A")"
INTERACT="$(valor_de "/silent-command remote.call('xfactorvr', 'interact_entity'" "$SALIDA_A")"
case "$TELEPORT" in
    # El mod responde con rcon.print("OK") y ademas devuelve true; vale cualquiera.
    *true*|*OK*) ok "el jugador real se teletransporta (teleport_player)" ;;
    *) fail "teleport_player no devolvio OK (obtenido: ${TELEPORT:-vacio})" ;;
esac
case "$INTERACT" in
    *true*|*OK*) ok "el jugador real interactua con entidades (interact_entity)" ;;
    *) fail "interact_entity no devolvio true (obtenido: ${INTERACT:-vacio})" ;;
esac

# --------------------------------------------------------------------------
# 5. Los logs: ni un error recuperable y cada proceso en su arbol de datos
# --------------------------------------------------------------------------
paso "Revision de los logs"

NON_REC_SRV="$(contiene 'non-recoverable' "$SERVER_LOG")"
NON_REC_CLI="$(contiene 'non-recoverable' "$CLIENT_LOG")"
if [ "${NON_REC_SRV:-0}" = "0" ] && [ "${NON_REC_CLI:-0}" = "0" ]; then
    ok "0 errores 'non-recoverable' en servidor y cliente"
else
    fail "errores 'non-recoverable': servidor=$NON_REC_SRV cliente=$NON_REC_CLI"
    in_container "grep -B2 -A 12 'non-recoverable' '$SERVER_LOG' | head -30" | sed 's/^/  /'
fi

W_SRV="$(in_container "grep -m1 'Write data path' '$SERVER_LOG' | sed 's/.*Write data path: //; s/ \\[.*//'")"
W_CLI="$(in_container "grep -m1 'Write data path' '$CLIENT_LOG' | sed 's/.*Write data path: //; s/ \\[.*//'")"
R_CLI="$(in_container "grep -m1 'Read data path' '$CLIENT_LOG' | sed 's/.*Read data path: //'")"
printf '  servidor: write-data=%s\n' "$W_SRV"
printf '  cliente : write-data=%s read-data=%s\n' "$W_CLI" "$R_CLI"
if [ -n "$W_CLI" ] && [ "$W_CLI" != "$W_SRV" ]; then
    ok "el cliente tiene su propio write-data (no pelean por el .lock)"
else
    fail "el cliente y el servidor comparten write-data ($W_SRV / $W_CLI)"
fi
if [ "$R_CLI" = "$FACTORIO_DIR/data" ]; then
    ok "el cliente comparte los datos de solo lectura del juego ($R_CLI)"
else
    fail "el cliente no encuentra los datos del juego (read-data=$R_CLI)"
fi

# El inyector (si se uso) tiene que seguir enganchado al render del cliente.
if [ -n "$PRELOAD_INYECTOR" ]; then
    HOOKS="$(in_container "grep -c 'hook vivo' '$CLIENT_STDOUT' 2>/dev/null || true" | tr -d '[:space:]')"
    if [ "${HOOKS:-0}" -gt 0 ] 2>/dev/null; then
        ok "el inyector sigue vivo dentro del cliente ($HOOKS avisos de hook)"
    else
        fail "el inyector no engancho el render del cliente"
    fi
fi

# --------------------------------------------------------------------------
# 6. Limpieza y veredicto
# --------------------------------------------------------------------------
paso "Limpieza"
in_container "pkill -f 'arm64/factor''io' >/dev/null 2>&1; sleep 1; true"
in_container "rm -f '$CMDS_A' '$CMDS_B' >/dev/null 2>&1; true"
ok "procesos detenidos"

echo
echo "==========================================================="
if [ "$FALLOS" -eq 0 ]; then
    echo "RESULTADO: CORRECTO (${PASOS} fases, 0 fallos)"
    echo "La campana se juega: servidor headless + cliente grafico MP."
    echo "==========================================================="
    exit 0
else
    echo "RESULTADO: FALLO (${PASOS} fases, ${FALLOS} fallos)"
    echo "Logs: $CONTAINER:$SERVER_LOG y $CONTAINER:$CLIENT_LOG"
    echo "==========================================================="
    exit 1
fi
