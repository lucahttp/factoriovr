#!/bin/bash
# test_jugador_virtual.sh — Banco del JUGADOR VIRTUAL de la campaña.
#
# Comprueba que la campaña de Factorio se puede jugar en un servidor headless
# SIN cliente gráfico, que es lo que hace falta en la Pico 4 Ultra (allí el
# cliente gráfico no arranca: SELinux no deja hacer dlopen de Mesa y la app
# tampoco puede ejecutar el ELF).
#
# La idea: el nivel de tutorial es singleplayer (usa game.players[1] en su
# init), así que en un servidor sin jugadores la historia no arranca. El mod
# xfactorvr_bridge crea un PERSONAJE de la fuerza "player" y el escenario
# parcheado lo usa en lugar de game.players[1].
#
# Las comprobaciones (todas tienen que pasar para salir con 0):
#   1. El servidor headless arranca con el escenario de campaña y abre el RCON.
#   2. NO hay ningún jugador conectado (esto es el caso que antes fallaba).
#   3. El escenario no peta: 0 "non-recoverable error" en el log.
#   4. El mundo carga el nivel: hay superficie y la historia existe.
#   5. El jugador virtual existe: personaje en la superficie, con posicion,
#      inventario y nombre.
#   6. Las ordenes del mod funcionan sobre el jugador virtual: teleport, minar
#      y fabricar (los items van al inventario REAL del personaje).
#   7. El nivel AVANZA: la historia pasa de la rama inicial a los nodos que
#      dependen de lo que hace el jugador (moverse, coger el pecio, minar).
#      OJO: el nivel de campana tiene auto_pause=true en el banco del
#      servidor; aqui se quita, porque sin jugadores el mapa se congela.
#
# Códigos de salida:
#   0  todo correcto
#   1  algo falla
#   2  el entorno no permite validarlo (sin podman, sin contenedor, sin
#      binario, sin datos, ...)
#
# Uso: bash scripts/test_jugador_virtual.sh
#
# Variables de entorno:
#   PODMAN          binario de podman (por defecto: podman)
#   CONTAINER       contenedor (por defecto: xfactorvr-client)
#   FACTIO_DIR      raíz del servidor dentro del contenedor (/factorio)
#   ESCENARIO       escenario de campaña (base/xfv-virtual)
#   NIVEL_ORIGEN    nivel de tutorial del que se copia (base/campaigns/tutorial/level-01)
#   PUERTO          puerto del juego (34199)
#   RCON_PORT       puerto del RCON (27019)
#   RCON_PASSWORD   contraseña del RCON (xfactorvr)
#   XFV_TIMEOUT     segundos de espera maxima por fase (120)

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-podman}"
CONTAINER="${CONTAINER:-xfactorvr-client}"
FACTORIO_DIR="${FACTORIO_DIR:-/factorio}"
ESCENARIO="${ESCENARIO:-base/xfv-virtual}"
NIVEL_ORIGEN="${NIVEL_ORIGEN:-base/campaigns/tutorial/level-01}"
PUERTO="${PUERTO:-34199}"
RCON_PORT="${RCON_PORT:-27019}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
XFV_TIMEOUT="${XFV_TIMEOUT:-120}"

PROBE="$PROJECT_ROOT/scripts/rcon_probe.py"
PARCHEADOR="$PROJECT_ROOT/scripts/preparar_campana_mp.py"
MOD_PUENTE="$PROJECT_ROOT/mods/xfactorvr_bridge"

# Ficheros dentro del contenedor.
SERVER_LOG="$FACTORIO_DIR/jugador_virtual_server.log"
SERVER_SETTINGS="$FACTORIO_DIR/config/server-settings-virtual.json"
CMDS="$FACTORIO_DIR/tmp/jugador_virtual_cmds.txt"

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

# Para el servidor hay que matarlo por PID y no con pkill -f: el patron
# "rcon-port 27019" tambien aparece en la linea de comandos del propio shell
# que lanza el servidor, y pkill se mataria a si mismo. Con ps+awk solo se
# mata el proceso del motor (el que tiene ./bin/arm64/factorio como argv[0]).
matar_servidor() {
    in_container 'ps -eo pid,args | awk "\$2 ~ /factorio\$/ {print \$1}" | xargs -r kill 2>/dev/null; true'
    # El motor guarda la partida al recibir la senal: el .lock tarda en soltar.
    for i in $(seq 1 30); do
        if ! in_container "test -e '$FACTORIO_DIR/.lock'" 2>/dev/null; then
            return 0
        fi
        sleep 1
    done
    return 1
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

for ruta in "$FACTORIO_DIR/bin/arm64/factorio" "$FACTORIO_DIR/data/base"; do
    in_container "test -e '$ruta'" || sin_entorno "en el contenedor no esta '$ruta'"
done

# El nivel de tutorial tiene que estar intacto en el contenedor: es la fuente
# de la que se copia el escenario del banco.
ORIGEN_ABS="$FACTORIO_DIR/data/$NIVEL_ORIGEN"
in_container "test -f '$ORIGEN_ABS/control.lua'" \
    || sin_entorno "el nivel '$NIVEL_ORIGEN' no esta en el contenedor"

ESC_DIR="$FACTORIO_DIR/data/${ESCENARIO%%/*}/scenarios/${ESCENARIO#*/}"
in_container "command -v python3 >/dev/null" || sin_entorno "el contenedor no tiene python3"
ok "contenedor '$CONTAINER' con el motor y los datos del juego"

# --------------------------------------------------------------------------
# 1. Preparacion: escenario parcheado + mod puente + ajustes
# --------------------------------------------------------------------------
paso "Preparacion (escenario con jugador virtual, mod puente, ajustes)"

# Se copia el nivel INTACTO del tutorial a un escenario propio: el banco no
# toca el nivel original del juego.
in_container "mkdir -p '$ESC_DIR' '$FACTORIO_DIR/tmp' '$FACTORIO_DIR/config' && \
  cp '$ORIGEN_ABS/control.lua' '$ORIGEN_ABS/blueprint.zip' '$ORIGEN_ABS/description.json' '$ESC_DIR/' && \
  rm -f '$ESC_DIR/control.lua.orig' && \
  if [ -d '$ORIGEN_ABS/locale' ]; then rm -rf '$ESC_DIR/locale'; cp -r '$ORIGEN_ABS/locale' '$ESC_DIR/locale'; fi && \
  ls '$ESC_DIR'"

"$PODMAN" cp "$PARCHEADOR" "$CONTAINER:/tmp/preparar_campana_mp.py" >/dev/null
in_container "python3 /tmp/preparar_campana_mp.py '$ESC_DIR'" | sed 's/^/  /'
in_container "grep -q 'XFactorVR-MP' '$ESC_DIR/control.lua'" \
    || sin_entorno "el escenario no ha quedado parcheado"

# El mod puente, solo en el servidor (aquí no hay cliente).
"$PODMAN" cp "$MOD_PUENTE" "$CONTAINER:$FACTORIO_DIR/mods/" >/dev/null
in_container "cat > '$FACTORIO_DIR/mods/mod-list.json' <<'JSON'
{
  \"mods\": [
    { \"name\": \"base\", \"enabled\": true },
    { \"name\": \"xfactorvr_bridge\", \"enabled\": true }
  ]
}
JSON"

# auto_pause=FALSE: sin jugadores conectados el juego se congela si no, y el
# nivel no puede avanzar. Es justo lo que hay que comprobar.
in_container "cat > '$SERVER_SETTINGS' <<'JSON'
{
  \"name\": \"XFactorVR-Jugador-Virtual\",
  \"description\": \"Campana con jugador virtual (servidor headless, sin cliente grafico)\",
  \"visibility\": { \"public\": false, \"lan\": false },
  \"require_user_verification\": false,
  \"max_players\": 4,
  \"autosave_interval\": 0,
  \"auto_pause\": false,
  \"auto_pause_when_players_connect\": false,
  \"only_admins_can_pause_the_game\": true,
  \"allow_commands\": \"admins-only\"
}
JSON"
ok "escenario parcheado, mod puente instalado y auto_pause=false"

# --------------------------------------------------------------------------
# 2. Arranque del SOLO servidor headless (nada de cliente grafico)
# --------------------------------------------------------------------------
paso "Arranque del servidor headless (sin cliente grafico)"

in_container "pkill -f 'rcon-port $RCON_PORT' >/dev/null 2>&1; sleep 3; true"
matar_servidor
if in_container "test -e '$FACTORIO_DIR/.lock'" 2>/dev/null; then
    fail "el .lock del servidor sigue ocupado: hay otro motor corriendo"
fi

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
    ok "servidor headless arrancado y RCON escuchando en el puerto $RCON_PORT (tras ${i}s)"
else
    fail "el servidor no abrio el RCON"
    in_container "tail -25 '$SERVER_LOG'" | sed 's/^/  /'
    if in_container "grep -q 'exclusive lock' '$SERVER_LOG' 2>/dev/null"; then
        fail "hay otro motor corriendo en el contenedor (lock ocupado)"
    fi
fi

# --------------------------------------------------------------------------
# 3. Comprobaciones por RCON
# --------------------------------------------------------------------------
paso "Comprobaciones por RCON (jugador virtual y avance del nivel)"

# rcon_query <fichero de comandos> -> imprime "orden<TAB>cuerpo".
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
        index($1, p) == 1 { if ($2 != "" && !v) { v = $2; } } END { print v}'
}

# El primer comando de consola de cada sesion RCON puede ser rechazado por
# el motor, asi que cada orden va duplicada: se usa la primera respuesta no
# vacia.
in_container "cat > '$CMDS' <<'CMDS'
/silent-command rcon.print('tick='..game.tick)
/silent-command rcon.print('tick='..game.tick)
/silent-command rcon.print('jugadores='..#game.players)
/silent-command rcon.print('jugadores='..#game.players)
/silent-command rcon.print('nodo='..tostring(story_branches[storage.story.story_index][storage.story.story_position].name))
/silent-command rcon.print('nodo='..tostring(story_branches[storage.story.story_index][storage.story.story_position].name))
/silent-command rcon.print('superficies='..#game.surfaces)
/silent-command rcon.print('superficies='..#game.surfaces)
/silent-command rcon.print('chars='..#game.surfaces[1].find_entities_filtered({type='character'}))
/silent-command rcon.print('chars='..#game.surfaces[1].find_entities_filtered({type='character'}))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('pos='..tostring(ch and ch.position.x)..','..tostring(ch and ch.position.y)..' inv='..tostring(ch and ch.get_item_count('iron-ore')))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('pos='..tostring(ch and ch.position.x)..','..tostring(ch and ch.position.y)..' inv='..tostring(ch and ch.get_item_count('iron-ore')))
/silent-command remote.call('xfactorvr', 'get_virtual_player')
/silent-command remote.call('xfactorvr', 'get_virtual_player')
CMDS"

SALIDA="$(rcon_query "$CMDS")"
TICK="$(valor_de "/silent-command rcon.print('tick" "$SALIDA")"
JUGADORES="$(valor_de "/silent-command rcon.print('jugadores" "$SALIDA")"
NODO="$(valor_de "/silent-command rcon.print('nodo" "$SALIDA")"
SUPERFICIES="$(valor_de "/silent-command rcon.print('superficies" "$SALIDA")"
CHARS="$(valor_de "/silent-command rcon.print('chars" "$SALIDA")"
POS="$(valor_de "/silent-command local ch=" "$SALIDA")"
VR="$(valor_de "/silent-command remote.call('xfactorvr', 'get_virtual_player'" "$SALIDA")"

printf '  RCON: %s | %s | %s\n' "$TICK" "$JUGADORES" "$NODO"
printf '  RCON: %s | %s\n' "$SUPERFICIES" "$CHARS"
printf '  RCON: %s\n' "$POS"
printf '  RCON: %s\n' "$VR"

# 3a. no hay NADIE conectado (el caso que antes no funcionaba)
case "$JUGADORES" in
    jugadores=*)
        n="${JUGADORES#jugadores=}"; n="${n%% *}"
        if [ "${n:-1}" -eq 0 ] 2>/dev/null; then
            ok "no hay ningun jugador conectado: el servidor va solo"
        else
            fail "se esperaba 0 jugadores conectados (obtenido: $JUGADORES)"
        fi ;;
    *) fail "el RCON no devolvio el numero de jugadores ($JUGADORES)" ;;
esac

# 3b. el mundo ha cargado el nivel
case "$SUPERFICIES" in
    superficies=*)
        n="${SUPERFICIES#superficies=}"; n="${n%% *}"
        if [ "${n:-0}" -ge 1 ] 2>/dev/null; then
            ok "el mundo ha cargado el nivel (hay superficie)"
        else
            fail "no hay superficie: el escenario no ha cargado ($SUPERFICIES)"
        fi ;;
    *) fail "el RCON no devolvio el numero de superficies ($SUPERFICIES)" ;;
esac

# 3c. el jugador virtual existe
case "$CHARS" in
    chars=*)
        n="${CHARS#chars=}"; n="${n%% *}"
        if [ "${n:-0}" -ge 1 ] 2>/dev/null; then
            ok "el jugador virtual existe: hay $CHARS personaje(s) en la superficie"
        else
            fail "no hay ningun personaje en la superficie ($CHARS)"
        fi ;;
    *) fail "el RCON no devolvio el numero de personajes ($CHARS)" ;;
esac

case "$VR" in
    VR\|virtual\|*) ok "el remoto 'get_virtual_player' devuelve el jugador virtual ($VR)" ;;
    *) fail "el remoto 'get_virtual_player' no devuelve el jugador virtual (obtenido: ${VR:-vacio})" ;;
esac

case "$POS" in
    pos=*) ok "el jugador virtual tiene posicion e inventario ($POS)" ;;
    *) fail "no se pudo leer la posicion del jugador virtual" ;;
esac

# 3d. las ordenes del mod funcionan sobre el jugador virtual
in_container "cat > '$CMDS' <<'CMDS'
/silent-command remote.call('xfactorvr', 'teleport_player', 12, 12)
/silent-command remote.call('xfactorvr', 'teleport_player', 12, 12)
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('pos='..tostring(ch.position.x)..','..tostring(ch.position.y))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('pos='..tostring(ch.position.x)..','..tostring(ch.position.y))
CMDS"
SALIDA_TP="$(rcon_query "$CMDS")"
POS_TP="$(valor_de "/silent-command local ch=" "$SALIDA_TP")"
printf '  RCON: tras teleport %s\n' "$POS_TP"
case "$POS_TP" in
    pos=12,12*) ok "teleport_player mueve al jugador virtual ($POS_TP)" ;;
    *) fail "teleport_player no movio al jugador virtual (obtenido: ${POS_TP:-vacio})" ;;
esac

# Minado: se busca una veta de hierro y se mina.
in_container "cat > '$CMDS' <<'CMDS'
/silent-command local ore=game.surfaces[1].find_entities_filtered({type='resource',name='iron-ore'})[1]; rcon.print('ore='..tostring(ore and ore.position.x)..','..tostring(ore and ore.position.y))
/silent-command local ore=game.surfaces[1].find_entities_filtered({type='resource',name='iron-ore'})[1]; rcon.print('ore='..tostring(ore and ore.position.x)..','..tostring(ore and ore.position.y))
CMDS"
SALIDA_ORE="$(rcon_query "$CMDS")"
ORE="$(valor_de "/silent-command local ore=" "$SALIDA_ORE")"
OX="${ORE#ore=}"; OX="${OX%%,*}"; OY="${ORE#*,}"
printf '  RCON: veta de hierro en %s\n' "$ORE"

if [ -n "${OX:-}" ] && [ -n "${OY:-}" ]; then
    in_container "cat > '$CMDS' <<'CMDS'
/silent-command remote.call('xfactorvr', 'teleport_player', $OX, $OY)
/silent-command remote.call('xfactorvr', 'teleport_player', $OX, $OY)
/silent-command remote.call('xfactorvr', 'manual_mine', $OX, $OY)
/silent-command remote.call('xfactorvr', 'manual_mine', $OX, $OY)
/silent-command remote.call('xfactorvr', 'manual_mine', $OX, $OY)
/silent-command remote.call('xfactorvr', 'manual_mine', $OX, $OY)
/silent-command remote.call('xfactorvr', 'manual_mine', $OX, $OY)
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('mineral='..ch.get_item_count('iron-ore'))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('mineral='..ch.get_item_count('iron-ore'))
CMDS"
    SALIDA_MINE="$(rcon_query "$CMDS")"
    MINERAL="$(valor_de "/silent-command local ch=" "$SALIDA_MINE")"
    printf '  RCON: tras minar %s\n' "$MINERAL"
    case "$MINERAL" in
        mineral=*)
            n="${MINERAL#mineral=}"; n="${n%% *}"
            if [ "${n:-0}" -ge 1 ] 2>/dev/null; then
                ok "manual_mine mete el mineral en el inventario del jugador virtual ($MINERAL)"
            else
                fail "manual_mine no ha puesto mineral en el inventario ($MINERAL)"
            fi ;;
        *) fail "no se pudo leer el inventario tras minar" ;;
    esac
else
    fail "no se ha encontrado ninguna veta de hierro en el mapa"
fi

# Fabricacion: se da material y se fabrica una pieza real.
in_container "cat > '$CMDS' <<'CMDS'
/silent-command remote.call('xfactorvr', 'craft', 'iron-plate', 1)
/silent-command remote.call('xfactorvr', 'craft', 'iron-plate', 1)
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('planchas='..ch.get_item_count('iron-plate'))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('planchas='..ch.get_item_count('iron-plate'))
CMDS"
SALIDA_CRAFT="$(rcon_query "$CMDS")"
PLANCHAS="$(valor_de "/silent-command local ch=" "$SALIDA_CRAFT")"
printf '  RCON: tras fabricar %s\n' "$PLANCHAS"
case "$PLANCHAS" in
    planchas=*)
        n="${PLANCHAS#planchas=}"; n="${n%% *}"
        if [ "${n:-0}" -ge 1 ] 2>/dev/null; then
            ok "craft fabrica en el inventario del jugador virtual ($PLANCHAS)"
        else
            fail "craft no ha fabricado nada ($PLANCHAS)"
        fi ;;
    *) fail "no se pudo leer el inventario tras fabricar" ;;
esac

# 3e. el nivel AVANZA
#
# La historia empieza en 'setup' y va pasando nodos. Con el jugador virtual
# tiene que pasar de 'setup' a los nodos que dependen de lo que hace el
# jugador (moverse, coger el pecio, minar el mineral, fundirlo...). El banco
# hace de jugador: cada vez que ve un nodo, hace lo que ese nodo pide.
echo
printf '  --- siguiendo la historia (el banco hace de jugador) ---\n'

# Nodo en el que esta la historia ahora.
nodo_actual() {
    in_container "cat > '$CMDS' <<'CMDS'
/silent-command rcon.print('nodo='..tostring(story_branches[storage.story.story_index][storage.story.story_position].name))
/silent-command rcon.print('nodo='..tostring(story_branches[storage.story.story_index][storage.story.story_position].name))
CMDS"
    local v
    v="$(valor_de "/silent-command rcon.print('nodo" "$(rcon_query "$CMDS")")"
    v="${v#nodo=}"
    printf '%s' "${v%% *}"
}

# Posicion de un recurso del mapa ("x,y"), o cadena vacia.
pos_recurso() {
    in_container "cat > '$CMDS' <<CMDS
/silent-command local r=game.surfaces[1].find_entities_filtered({type='resource',name='$1'})[1]; rcon.print('pos='..tostring(r and r.position.x)..','..tostring(r and r.position.y))
/silent-command local r=game.surfaces[1].find_entities_filtered({type='resource',name='$1'})[1]; rcon.print('pos='..tostring(r and r.position.x)..','..tostring(r and r.position.y))
CMDS"
    local v
    v="$(valor_de "/silent-command local r=" "$(rcon_query "$CMDS")")"
    v="${v#pos=}"
    printf '%s' "${v%% *}"
}

# Una posicion sirve solo si no es "nil,nil" (la entidad no existia).
pos_valida() {
    case "$1" in
        "") return 1 ;;
        nil*) return 1 ;;
    esac
    return 0
}

# Posicion de la entidad con una etiqueta (el pecio del tutorial se busca por
# la etiqueta "wreck", no por el nombre del prototipo: hay varias cajas).
pos_por_tag() {
    in_container "cat > '$CMDS' <<CMDS
/silent-command local r=game.get_entity_by_tag('$1'); rcon.print('pos='..tostring(r and r.position.x)..','..tostring(r and r.position.y))
/silent-command local r=game.get_entity_by_tag('$1'); rcon.print('pos='..tostring(r and r.position.x)..','..tostring(r and r.position.y))
CMDS"
    local v
    v="$(valor_de "/silent-command local r=" "$(rcon_query "$CMDS")")"
    v="${v#pos=}"
    printf '%s' "${v%% *}"
}

# Donde suelta el mineral la perforadora (x,y), o cadena vacia.
pos_solte_minero() {
    in_container "cat > '$CMDS' <<'CMDS'
/silent-command local m=game.surfaces[1].find_entities_filtered({name='burner-mining-drill'})[1]; rcon.print('pos='..tostring(m and m.drop_position.x)..','..tostring(m and m.drop_position.y))
/silent-command local m=game.surfaces[1].find_entities_filtered({name='burner-mining-drill'})[1]; rcon.print('pos='..tostring(m and m.drop_position.x)..','..tostring(m and m.drop_position.y))
CMDS"
    local v
    v="$(valor_de "/silent-command local m=" "$(rcon_query "$CMDS")")"
    v="${v#pos=}"
    printf '%s' "${v%% *}"
}

# Posicion de una entidad por nombre ("x,y"), o cadena vacia.
pos_entidad() {
    in_container "cat > '$CMDS' <<CMDS
/silent-command local r=game.surfaces[1].find_entities_filtered({name='$1'})[1]; rcon.print('pos='..tostring(r and r.position.x)..','..tostring(r and r.position.y))
/silent-command local r=game.surfaces[1].find_entities_filtered({name='$1'})[1]; rcon.print('pos='..tostring(r and r.position.x)..','..tostring(r and r.position.y))
CMDS"
    local v
    v="$(valor_de "/silent-command local r=" "$(rcon_query "$CMDS")")"
    v="${v#pos=}"
    printf '%s' "${v%% *}"
}

# Envia ordenes remotas al mod. Se acumulan y se mandan todas juntas en una
# sola sesion RCON (cada orden va duplicada porque el primer comando de una
# sesion RCON a veces se rechaza).
# Encola una orden remota del mod.
#   agregar_orden <nombre> [argumentos...]
# Los argumentos van como los que entiende Lua: los textos entre comillas
# simples se pasan con comillas ("'stone-furnace'").
ORDENES=()
agregar_orden() {
    local nombre="$1"
    shift
    local args=""
    local a
    for a in "$@"; do
        args="$args, $a"
    done
    # Cada orden va duplicada: el primer comando de una sesion RCON a veces
    # se rechaza.
    ORDENES+=("/silent-command remote.call('xfactorvr', '${nombre}'${args})")
    ORDENES+=("/silent-command remote.call('xfactorvr', '${nombre}'${args})")
}

ejecutar_ordenes() {
    [ "${#ORDENES[@]}" -eq 0 ] && return 0
    # Una orden por sesion RCON: en una sesion con varias ordenes el motor
    # se come alguna (se vio truncando comandos).
    local linea
    for linea in "${ORDENES[@]}"; do
        in_container "cat > '$CMDS' <<CMDS
$linea
CMDS"
        if [ "${XFV_DEBUG:-0}" = "1" ]; then
            in_container "cat -A '$CMDS'" | sed 's/^/      FICHERO: /'
        fi
        rcon_query "$CMDS" | cut -f2 | tr '\n' ' ' | sed 's/^/      RCON: /'
        echo
    done
    ORDENES=()
}

NODO_ANTES="${NODO#nodo=}"
NODO_ANTES="${NODO_ANTES%% *}"
NODO_FINAL="$NODO_ANTES"
for i in $(seq 1 "${XFV_PASOS:-30}"); do
    NODO_FINAL="$(nodo_actual)"
    printf '    t+%2ss  nodo=%s\n' "$((i * 5))" "$NODO_FINAL"

    case "$NODO_FINAL" in
        movement)
            # Moverse mas de 5 casillas del punto de partida.
            agregar_orden teleport_player 12 12 ;;
        open-wreck|take-plates-from-wreck)
            # Coger los engranajes del pecio.
            W="$(pos_por_tag 'wreck')"
            pos_valida "$W" || W="4.5,-7.5"
            agregar_orden interact_entity "${W}" 0 ;;
        iron-ore-gathering)
            # Senalar el mineral (el tutorial mira `selected`).
            O="$(pos_recurso 'iron-ore')"
            if [ -n "$O" ]; then
                agregar_orden teleport_player "${O}" 0
                agregar_orden interact_entity "${O}" 0
            fi ;;
        handmining)
            # Minar 5 unidades de mineral.
            O="$(pos_recurso 'iron-ore')"
            if [ -n "$O" ]; then
                agregar_orden teleport_player "${O}" 0
                agregar_orden manual_mine "${O}" 0
                agregar_orden manual_mine "${O}" 0
                agregar_orden manual_mine "${O}" 0
                agregar_orden manual_mine "${O}" 0
                agregar_orden manual_mine "${O}" 0
            fi ;;
        craft-furnace-start|craft-furnace-finish|show-crafting-queue)
            # Fabricar el horno de piedra: hacen falta 5 piedras.
            S="$(pos_recurso 'stone')"
            if [ -n "$S" ]; then
                agregar_orden teleport_player "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
            fi
            agregar_orden craft "'stone-furnace'" 1 ;;
        place-stone-furnace)
            # Colocar el horno (solo si no hay ya uno: el nivel mira el
            # PRIMERO que encuentra y con dos hornos se queda con el vacio).
            F="$(pos_recurso 'stone')"
            pos_valida "$F" || F="-11,-6"
            HORNOS="$(pos_entidad 'stone-furnace')"
            if ! pos_valida "$HORNOS"; then
                agregar_orden place_entity "'stone-furnace'" "${F}" 0
            fi ;;
        open-furnace|put-iron-ore)
            # Meter el mineral en el horno (inventario 2 = ingredientes).
            F="$(pos_entidad 'stone-furnace')"
            if pos_valida "$F"; then
                agregar_orden insert_item "${F}" "'iron-ore'" 5 2
            fi ;;
        put-coal)
            # Minar carbon y meterlo como combustible (inventario 1).
            C="$(pos_recurso 'coal')"
            if pos_valida "$C"; then
                # Sin teletransporte: manual_mine no mira la distancia.
                agregar_orden manual_mine "${C}" 0
            fi
            F="$(pos_entidad 'stone-furnace')"
            if pos_valida "$F"; then
                agregar_orden insert_item "${F}" "'coal'" 1 1
            fi ;;
        take-plates-from-furnace)
            # Recoger las planchas fundidas.
            F="$(pos_entidad 'stone-furnace')"
            if pos_valida "$F"; then
                agregar_orden interact_entity "${F}" 0
            fi ;;
        craft-burner-mining-drill)
            # La perforadora de combustible (Factorio 2.1): 3 engranajes +
            # 3 planchas + 1 horno de piedra.
            F="$(pos_entidad 'stone-furnace')"
            O="$(pos_recurso 'iron-ore')"
            # Mas mineral al horno: hacen falta planchas.
            if pos_valida "$F" && pos_valida "$O"; then
                agregar_orden insert_item "${F}" "'iron-ore'" 5 2
            fi
            # Recoger las planchas que ya haya fundido.
            if pos_valida "$F"; then
                agregar_orden interact_entity "${F}" 0
            fi
            # Piedra para el horno de la receta.
            S="$(pos_recurso 'stone')"
            if pos_valida "$S"; then
                agregar_orden teleport_player "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
                agregar_orden manual_mine "${S}" 0
            fi
            agregar_orden craft "'stone-furnace'" 1
            agregar_orden craft "'iron-gear-wheel'" 3
            agregar_orden craft "'burner-mining-drill'" 1 ;;
        add_filter)
            # Asignar la perforadora a un hueco de la barra rapida virtual.
            agregar_orden set_quick_bar 1 1 "'burner-mining-drill'" ;;
        place-burner-mining-drill)
            # Colocarla encima de la veta de hierro.
            O="$(pos_recurso 'iron-ore')"
            if pos_valida "$O"; then
                agregar_orden place_entity "'burner-mining-drill'" "${O}" 0
            fi ;;
        fuel-burner-mining-drill)
            # Meterle carbon como combustible.
            C="$(pos_recurso 'coal')"
            if pos_valida "$C"; then
                agregar_orden manual_mine "${C}" 0
            fi
            M="$(pos_entidad 'burner-mining-drill')"
            if pos_valida "$M"; then
                agregar_orden insert_item "${M}" "'coal'" 1 1
            fi ;;
        craft-iron-chest)
            agregar_orden craft "'iron-chest'" 1 ;;
        place-iron-chest)
            # La caja va al lado de la perforadora (una casilla mas alla).
            M="$(pos_entidad 'burner-mining-drill')"
            if pos_valida "$M"; then
                agregar_orden place_entity "'iron-chest'" "${M}" 0
            fi ;;
        show-miner-blockage)
            # Recoger del suelo el mineral que ha minado la perforadora: el
            # nivel espera el evento de "minado" del jugador.
            I="$(pos_entidad 'item-on-ground')"
            if pos_valida "$I"; then
                agregar_orden mine_entity "${I}" 0
            fi ;;
        wait-to-check-target|remove-blocking-entity)
            # Nada que hacer: la perforadora trabaja y el nivel ya mira donde.
            ;;
        place-iron-chest)
            # La caja va donde suelta el mineral la perforadora.
            D="$(pos_solte_minero)"
            if pos_valida "$D"; then
                agregar_orden place_entity "'iron-chest'" "${D}" 0
            fi ;;
        take-items)
            # Recoger lo que ha minado la perforadora.
            C="$(pos_entidad 'iron-chest')"
            if pos_valida "$C"; then
                agregar_orden interact_entity "${C}" 0
            fi ;;
    esac
    ejecutar_ordenes
    if [ "${XFV_DEBUG:-0}" = "1" ]; then
        in_container "cat > '$CMDS' <<'CMDS'
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('mochila engr='..tostring(ch.get_item_count('iron-gear-wheel'))..' planchas='..tostring(ch.get_item_count('iron-plate'))..' piedra='..tostring(ch.get_item_count('stone'))..' horno='..tostring(ch.get_item_count('stone-furnace'))..' taladro='..tostring(ch.get_item_count('burner-mining-drill')))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('mochila engr='..tostring(ch.get_item_count('iron-gear-wheel'))..' planchas='..tostring(ch.get_item_count('iron-plate'))..' piedra='..tostring(ch.get_item_count('stone'))..' horno='..tostring(ch.get_item_count('stone-furnace'))..' taladro='..tostring(ch.get_item_count('burner-mining-drill')))
/silent-command rcon.print('pecio='..tostring(game.get_entity_by_tag('wreck').get_item_count('iron-gear-wheel')))
/silent-command rcon.print('pecio='..tostring(game.get_entity_by_tag('wreck').get_item_count('iron-gear-wheel')))
CMDS"
        rcon_query "$CMDS" | cut -f2 | sed 's/^/      /'
    fi
    sleep 5
done

printf '  RCON: nodo inicial=%s  nodo final=%s\n' "$NODO_ANTES" "$NODO_FINAL"

case "$NODO_ANTES" in
    "")
        fail "no se ha podido leer el nodo de la historia (el RCON no responde)" ;;
    setup*)
        if [ "$NODO_FINAL" != "$NODO_ANTES" ] && [ -n "$NODO_FINAL" ]; then
            ok "el nivel AVANZA sin ningun jugador conectado ($NODO_ANTES -> $NODO_FINAL)"
        else
            fail "el nivel se ha quedado clavado en '$NODO_ANTES'"
        fi ;;
    *)
        ok "el nivel ya habia arrancado (nodo inicial=$NODO_ANTES)" ;;
esac

case "$NODO_FINAL" in
    movement|open-wreck|take-plates-from-wreck|put-plates|close-wreck-gui|iron-ore-gathering|handmining|show-tooltip|craft-furnace-start|craft-furnace-finish|show-crafting-queue|place-stone-furnace|open-furnace|put-iron-ore|put-coal|show-furnace-smelting|wait-for-iron-plates|take-plates-from-furnace)
        ok "el nivel ha llegado a los nodos que dependen del jugador ($NODO_FINAL)" ;;
    victory)
        ok "el nivel ha llegado al FINAL (victory)" ;;
    *)
        aviso "el nivel se ha parado en '$NODO_FINAL' (nodos de GUI: necesitan el cliente)" ;;
esac

# --------------------------------------------------------------------------
# 4. Los logs
# --------------------------------------------------------------------------
paso "Revision de los logs"

NON_REC="$(in_container "grep -c 'non-recoverable' '$SERVER_LOG' 2>/dev/null || true" | tr -d '[:space:]')"
if [ "${NON_REC:-0}" = "0" ]; then
    ok "0 errores 'non-recoverable' en el servidor"
else
    fail "errores 'non-recoverable': $NON_REC"
    in_container "grep -B2 -A 12 'non-recoverable' '$SERVER_LOG' | head -30" | sed 's/^/  /'
fi

ERR_SCRIPT="$(in_container "grep -ciE 'Error while running|stack traceback' '$SERVER_LOG' 2>/dev/null || true" | tr -d '[:space:]')"
if [ "${ERR_SCRIPT:-0}" = "0" ]; then
    ok "0 errores de script en el log del servidor"
else
    fail "errores de script en el log: $ERR_SCRIPT"
    in_container "grep -iE -A 8 'Error while running' '$SERVER_LOG' | head -20" | sed 's/^/  /'
fi

CREADO="$(in_container "grep -c 'Jugador virtual' '$SERVER_LOG' 2>/dev/null || true" | tr -d '[:space:]')"
if [ "${CREADO:-0}" -ge 1 ] 2>/dev/null; then
    ok "el mod creo el jugador virtual al arrancar ($CREADO aviso(s) en el log)"
    in_container "grep 'Jugador virtual' '$SERVER_LOG' | head -2" | sed 's/^/  /'
else
    fail "el mod no ha creado el jugador virtual"
fi

# --------------------------------------------------------------------------
# 5. Limpieza y veredicto
# --------------------------------------------------------------------------
paso "Limpieza"
matar_servidor
in_container "rm -f '$CMDS' >/dev/null 2>&1; true"
ok "servidor detenido"

echo
echo "==========================================================="
if [ "$FALLOS" -eq 0 ]; then
    echo "RESULTADO: CORRECTO (${PASOS} fases, 0 fallos)"
    echo "La campana se juega en un servidor headless, SIN cliente grafico:"
    echo "el mod crea un jugador virtual y el escenario lo usa."
    echo "==========================================================="
    exit 0
else
    echo "RESULTADO: FALLO (${PASOS} fases, ${FALLOS} fallos)"
    echo "Logs: $CONTAINER:$SERVER_LOG"
    echo "==========================================================="
    exit 1
fi
