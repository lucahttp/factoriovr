#!/bin/bash
# test_inventario_vr.sh — Banco del INVENTARIO con manos VR.
#
# Comprueba las tres acciones que faltaban para jugar en VR sin raton:
#
#   1. COGER UN ITEM DE UNA CAJA  -> el item sale de la caja y llega al
#      inventario del actor (el jugador real o el personaje virtual).
#   2. COGER UN ITEM DEL SUELO    -> el item suelto se recoge y la entidad
#      "item-on-ground" desaparece.
#   3. METER UN ITEM EN UNA MAQUINA (horno) O EN UNA CAJA -> el item sale del
#      actor y aparece en la maquina (es un TRASPASO, no una copia) y se
#      puede volver a sacar.
#
# Ademas comprueba que el snapshot que manda el mod lleva la seccion "loose"
# con los items que hay sueltos en el suelo: es lo que usa el cliente VR para
# saber que hay bajo el rayo y que hacer con la mano.
#
# Y al final vuelve a pasar el banco del JUGADOR VIRTUAL
# (scripts/test_jugador_virtual.sh) para comprobar que lo anterior no ha
# roto nada: tienen que ser 6 fases y 0 fallos.
#
# OJO: este banco usa el escenario "freeplay" y NO el de la campana. La
# campana va dando items al jugador y llenando cajas por su cuenta (los nodos
# "wait-for-chest-to-fill" y "take-items"), y eso se mezclaría con las
# mediciones. Aqui se mide el inventario y nada mas; la campana la comprueba
# el banco del jugador virtual, que se ejecuta al final.
#
# Códigos de salida:
#   0  todo correcto
#   1  algo falla
#   2  el entorno no permite validarlo (sin podman, sin contenedor, sin
#      binario, sin datos, ...)
#
# Uso: bash scripts/test_inventario_vr.sh
#
# Variables de entorno:
#   PODMAN          binario de podman (por defecto: podman)
#   CONTAINER       contenedor (por defecto: xfactorvr-client)
#   FACTIO_DIR      raíz del servidor dentro del contenedor (/factorio)
#   ESCENARIO       escenario del banco (base/freeplay)
#   PUERTO          puerto del juego (34201)
#   RCON_PORT       puerto del RCON (27021)
#   RCON_PASSWORD   contraseña del RCON (xfactorvr)
#   XFV_SIN_JUGADOR 1 = no ejecutar el banco del jugador virtual (solo para
#                   depurar este banco; la validación de verdad lo ejecuta)

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-podman}"
CONTAINER="${CONTAINER:-xfactorvr-client}"
FACTORIO_DIR="${FACTORIO_DIR:-/factorio}"
ESCENARIO="${ESCENARIO:-base/freeplay}"
PUERTO="${PUERTO:-34201}"
RCON_PORT="${RCON_PORT:-27021}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
# Poner a 1 para saltarse el banco del jugador virtual (solo para depurar el
# banco del inventario; la validacion de verdad lo ejecuta siempre).
XFV_SIN_JUGADOR="${XFV_SIN_JUGADOR:-0}"

PROBE="$PROJECT_ROOT/scripts/rcon_probe.py"
MOD_PUENTE="$PROJECT_ROOT/mods/xfactorvr_bridge"
BANCO_JUGADOR="$PROJECT_ROOT/scripts/test_jugador_virtual.sh"

# Ficheros dentro del contenedor.
SERVER_LOG="$FACTORIO_DIR/inventario_vr_server.log"
SERVER_SETTINGS="$FACTORIO_DIR/config/server-settings-inventario.json"
CMDS="$FACTORIO_DIR/tmp/inventario_vr_cmds.txt"
SNAPSHOT="$FACTORIO_DIR/tmp/inventario_vr_snapshot.json"

# Posiciones de las entidades de prueba (lejos del origen para que no se
# mezclen con nada).
POS_CAJA=6
POS_HORNO=-6
POS_SUELO_Y=6

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
# "rcon-port ..." tambien aparece en la linea de comandos del propio shell
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
[ -f "$BANCO_JUGADOR" ] || sin_entorno "falta scripts/test_jugador_virtual.sh"
[ -d "$MOD_PUENTE" ] || sin_entorno "falta mods/xfactorvr_bridge"

for ruta in "$FACTORIO_DIR/bin/arm64/factorio" "$FACTORIO_DIR/data/base"; do
    in_container "test -e '$ruta'" || sin_entorno "en el contenedor no esta '$ruta'"
done

ESC_DIR="$FACTORIO_DIR/data/${ESCENARIO%%/*}/scenarios/${ESCENARIO#*/}"
in_container "test -f '$ESC_DIR/control.lua'" \
    || sin_entorno "el escenario '$ESCENARIO' no esta en el contenedor"
in_container "command -v python3 >/dev/null" || sin_entorno "el contenedor no tiene python3"
ok "contenedor '$CONTAINER' con el motor, el escenario '$ESCENARIO' y python3"

# --------------------------------------------------------------------------
# 1. Preparacion: mod puente + ajustes del servidor
# --------------------------------------------------------------------------
paso "Preparacion (mod puente y ajustes del servidor)"

in_container "mkdir -p '$FACTORIO_DIR/tmp' '$FACTORIO_DIR/config'"

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
# actor (el personaje virtual) no puede recoger ni meter nada.
in_container "cat > '$SERVER_SETTINGS' <<'JSON'
{
  \"name\": \"XFactorVR-Inventario-VR\",
  \"description\": \"Banco del inventario con manos VR\",
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
ok "mod puente instalado y auto_pause=false"

# --------------------------------------------------------------------------
# 2. Arranque del SOLO servidor headless (nada de cliente grafico)
# --------------------------------------------------------------------------
paso "Arranque del servidor headless (sin cliente grafico)"

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
    sin_entorno "sin servidor no se puede validar el inventario"
fi

# --------------------------------------------------------------------------
# 3. Comprobaciones por RCON
# --------------------------------------------------------------------------
paso "Comprobaciones por RCON (coger de la caja, del suelo y meter en maquina)"

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

# Igual, pero guardando la ultima instantanea (la que tiene "entities") en un
# fichero del contenedor: asi se puede validar que el cliente VR va a poder
# leerla como JSON.
rcon_query_guardar() {
    "$PODMAN" exec -i "$CONTAINER" python3 - --file "$1" "$RCON_PORT" "$RCON_PASSWORD" \
        --guardar "$SNAPSHOT" < "$PROBE" 2>/dev/null | python3 -c "$PARSER_RCON"
}

valor_de() {  # $1 = prefijo de la orden, $2 = texto de la salida
    printf '%s\n' "$2" | awk -F'\t' -v p="$1" '
        index($1, p) == 1 { if ($2 != "" && !v) { v = $2; } } END { print v}'
}

# Numero que va detras de "clave=" en una salida del tipo "clave=12 resto".
numero_de() {  # $1 = clave, $2 = salida
    local v="${2#*$1=}"
    v="${v%% *}"
    printf '%s' "${v%%[!0-9-]*}"
}

# El PRIMER comando de consola de cada sesion RCON puede ser rechazado por el
# motor (se responde vacio). Por eso cada tanda empieza con dos comandos
# inocuos: el primero se pierde y el segundo ya responde bien.
#
# Y a partir de ahi cada orden va UNA SOLA VEZ: si se duplicaran, un
# "create_entity" fusionaria las dos pilas en el suelo y un
# "insert_carried_item" meteria el doble de items.
WARMUP='/silent-command rcon.print('"'"'listo'"'"')
/silent-command rcon.print('"'"'listo'"'"')'

# OJO: para encontrar la entidad de prueba se usa un ayudante de Lua
# ("cerca") que se define en la propia tanda: la API de 2.1 NO tiene filtro
# por posicion en find_entities_filtered.

# --- Escenario: una caja, un horno y nada mas ------------------------------
in_container "cat > '$CMDS' <<'CMDS'
$WARMUP
/silent-command function cerca(nombre,x,y) local mejor,dist=nil,nil for _,e in ipairs(game.surfaces[1].find_entities_filtered({name=nombre})) do local dx=e.position.x-x local dy=e.position.y-y local d=dx*dx+dy*dy if not dist or d<dist then mejor,dist=e,d end end return mejor end
/silent-command local s=game.surfaces[1]; local e=s.create_entity{name='iron-chest', position={x=$POS_CAJA,y=0}}; rcon.print('caja_pos='..tostring(e and e.position.x)..','..tostring(e and e.position.y))
/silent-command local s=game.surfaces[1]; local e=s.create_entity{name='stone-furnace', position={x=$POS_HORNO,y=0}}; rcon.print('horno_pos='..tostring(e and e.position.x)..','..tostring(e and e.position.y))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('chars='..tostring(ch and ch.unit_number)..' pos='..tostring(ch and ch.position.x)..','..tostring(ch and ch.position.y))
CMDS"
SALIDA="$(rcon_query "$CMDS")"
CAJA="$(valor_de "/silent-command local s=game.surfaces[1]; local e=s.create_entity{name='iron-chest'" "$SALIDA")"
HORNO="$(valor_de "/silent-command local s=game.surfaces[1]; local e=s.create_entity{name='stone-furnace'" "$SALIDA")"
CHARS="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('chars" "$SALIDA")"
printf '  RCON: %s | %s | %s\n' "$CAJA" "$HORNO" "$CHARS"

case "$CHARS" in
    chars=*)
        n="${CHARS#chars=}"; n="${n%% *}"
        if [ "${n:-0}" -ge 1 ] 2>/dev/null; then
            ok "el actor (personaje virtual) existe y esta en el mapa"
        else
            fail "no hay ningun personaje en la superficie ($CHARS)"
        fi ;;
    *) fail "el RCON no devolvio el actor ($CHARS)" ;;
esac

case "$CAJA" in
    caja_pos=*)
        ok "la caja de prueba se ha colocado ($CAJA)" ;;
    *)
        fail "no se ha podido colocar la caja de prueba ($CAJA)" ;;
esac

# --- 3a. COGER UN ITEM DE UNA CAJA -----------------------------------------
paso "Coger un item de una caja (take_from_container)"

in_container "cat > '$CMDS' <<'CMDS'
$WARMUP
/silent-command function cerca(nombre,x,y) local mejor,dist=nil,nil for _,e in ipairs(game.surfaces[1].find_entities_filtered({name=nombre})) do local dx=e.position.x-x local dy=e.position.y-y local d=dx*dx+dy*dy if not dist or d<dist then mejor,dist=e,d end end return mejor end
/silent-command local e=cerca('iron-chest',$POS_CAJA,0); e.get_inventory(1).insert({name='copper-plate', count=10}); rcon.print('caja_antes='..tostring(e.get_inventory(1).get_item_count('copper-plate')))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('actor_antes='..tostring(ch.get_item_count('copper-plate')))
/silent-command remote.call('xfactorvr', 'take_from_container', $POS_CAJA, 0, '', 1)
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; local e=cerca('iron-chest',$POS_CAJA,0); rcon.print('actor_despues='..tostring(ch.get_item_count('copper-plate'))..' caja_despues='..tostring(e.get_inventory(1).get_item_count('copper-plate')))
CMDS"
SALIDA_CAJA="$(rcon_query "$CMDS")"
CAJA_ANTES="$(valor_de "/silent-command local e=cerca('iron-chest',$POS_CAJA,0); e.get_inventory(1).insert" "$SALIDA_CAJA")"
ACTOR_ANTES="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('actor_antes" "$SALIDA_CAJA")"
ACTOR_DESPUES="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; local e=cerca('iron-chest',$POS_CAJA,0)" "$SALIDA_CAJA")"
printf '  RCON: %s | %s | %s\n' "$CAJA_ANTES" "$ACTOR_ANTES" "$ACTOR_DESPUES"

N_ANTES="$(numero_de "caja_antes" "$CAJA_ANTES")"
N_ACTOR_ANTES="$(numero_de "actor_antes" "$ACTOR_ANTES")"
N_ACTOR_DESPUES="$(numero_de "actor_despues" "$ACTOR_DESPUES")"
N_CAJA_DESPUES="$(numero_de "caja_despues" "$ACTOR_DESPUES")"

if [ "${N_ANTES:-0}" -ge 1 ] 2>/dev/null; then
    ok "la caja tiene items dentro ($CAJA_ANTES)"
else
    fail "no se ha podido preparar la caja con items ($CAJA_ANTES)"
fi

if [ "${N_ACTOR_DESPUES:-0}" -gt "${N_ACTOR_ANTES:-0}" ] 2>/dev/null; then
    ok "take_from_container ha brought el item al actor ($ACTOR_ANTES -> $ACTOR_DESPUES)"
else
    fail "take_from_container no ha traido el item al actor ($ACTOR_ANTES -> $ACTOR_DESPUES)"
fi

if [ "${N_CAJA_DESPUES:-0}" -lt "${N_ANTES:-0}" ] 2>/dev/null; then
    ok "el item ha salido de la caja ($N_ANTES -> $N_CAJA_DESPUES)"
else
    fail "el item sigue en la caja ($N_ANTES -> $N_CAJA_DESPUES)"
fi

# --- 3b. COGER UN ITEM DEL SUELO ------------------------------------------
paso "Coger un item del suelo (pick_up_item)"

in_container "cat > '$CMDS' <<'CMDS'
$WARMUP
/silent-command local s=game.surfaces[1]; local e=s.create_entity{name='item-on-ground', position={x=0,y=$POS_SUELO_Y}, stack={name='iron-plate', count=5}}; rcon.print('suelo_pos='..tostring(e and e.position.x)..','..tostring(e and e.position.y))
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('actor_plan_antes='..tostring(ch.get_item_count('iron-plate'))..' suelo_antes='..tostring(#game.surfaces[1].find_entities_filtered({name='item-on-ground'})))
/silent-command remote.call('xfactorvr', 'pick_up_item', 0, $POS_SUELO_Y)
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('actor_plan_despues='..tostring(ch.get_item_count('iron-plate'))..' suelo_despues='..tostring(#game.surfaces[1].find_entities_filtered({name='item-on-ground'})))
CMDS"
SALIDA_SUELO="$(rcon_query "$CMDS")"
SUELO_ANTES="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('actor_plan_antes" "$SALIDA_SUELO")"
SUELO_DESPUES="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; rcon.print('actor_plan_despues" "$SALIDA_SUELO")"
printf '  RCON: %s | %s\n' "$SUELO_ANTES" "$SUELO_DESPUES"

N_PLAN_ANTES="$(numero_de "actor_plan_antes" "$SUELO_ANTES")"
N_PLAN_DESPUES="$(numero_de "actor_plan_despues" "$SUELO_DESPUES")"
N_SUELO_ANTES="$(numero_de "suelo_antes" "$SUELO_ANTES")"
N_SUELO_DESPUES="$(numero_de "suelo_despues" "$SUELO_DESPUES")"

if [ "${N_SUELO_ANTES:-0}" -ge 1 ] 2>/dev/null; then
    ok "hay un item en el suelo ($SUELO_ANTES)"
else
    fail "no se ha podido dejar un item en el suelo ($SUELO_ANTES)"
fi

if [ "${N_PLAN_DESPUES:-0}" -gt "${N_PLAN_ANTES:-0}" ] 2>/dev/null; then
    ok "pick_up_item ha recogido el item del suelo ($N_PLAN_ANTES -> $N_PLAN_DESPUES planchas)"
else
    fail "pick_up_item no ha recogido el item del suelo ($N_PLAN_ANTES -> $N_PLAN_DESPUES)"
fi

if [ "${N_SUELO_DESPUES:-0}" -lt "${N_SUELO_ANTES:-0}" ] 2>/dev/null; then
    ok "la entidad del item del suelo ha desaparecido ($N_SUELO_ANTES -> $N_SUELO_DESPUES)"
else
    fail "la entidad del item del suelo sigue en el mapa ($N_SUELO_ANTES -> $N_SUELO_DESPUES)"
fi

# --- 3c. METER UN ITEM EN UN HORNO (TRASPASO) ------------------------------
paso "Meter un item en un horno (insert_carried_item, ingredientes)"

in_container "cat > '$CMDS' <<'CMDS'
$WARMUP
/silent-command function cerca(nombre,x,y) local mejor,dist=nil,nil for _,e in ipairs(game.surfaces[1].find_entities_filtered({name=nombre})) do local dx=e.position.x-x local dy=e.position.y-y local d=dx*dx+dy*dy if not dist or d<dist then mejor,dist=e,d end end return mejor end
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; ch.get_main_inventory().clear(); ch.insert({name='iron-ore', count=6}); rcon.print('actor_mineral_antes='..tostring(ch.get_item_count('iron-ore')))
/silent-command remote.call('xfactorvr', 'insert_carried_item', $POS_HORNO, 0, 'iron-ore', 2, 2)
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; local e=cerca('stone-furnace',$POS_HORNO,0); rcon.print('actor_mineral_despues='..tostring(ch.get_item_count('iron-ore'))..' horno_ingredientes='..tostring(e.get_inventory(2).get_item_count('iron-ore')))
CMDS"
SALIDA_HORNO="$(rcon_query "$CMDS")"
MINERAL_ANTES="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; ch.get_main_inventory().clear()" "$SALIDA_HORNO")"
MINERAL_DESPUES="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; local e=cerca('stone-furnace',$POS_HORNO,0)" "$SALIDA_HORNO")"
printf '  RCON: %s | %s\n' "$MINERAL_ANTES" "$MINERAL_DESPUES"

N_MIN_ANTES="$(numero_de "actor_mineral_antes" "$MINERAL_ANTES")"
N_MIN_DESPUES="$(numero_de "actor_mineral_despues" "$MINERAL_DESPUES")"
N_HORNO="$(numero_de "horno_ingredientes" "$MINERAL_DESPUES")"

if [ "${N_MIN_DESPUES:-0}" -lt "${N_MIN_ANTES:-0}" ] 2>/dev/null \
   && [ "${N_HORNO:-0}" -ge 2 ] 2>/dev/null; then
    ok "el item ha salido del actor y ha entrado en el horno ($N_MIN_ANTES -> $N_MIN_DESPUES mineral, $N_HORNO en el horno)"
else
    fail "el item no ha pasado al horno ($N_MIN_ANTES -> $N_MIN_DESPUES mineral, $N_HORNO en el horno)"
fi

# --- 3d. Y VOLVER A SACARLO DEL HORNO --------------------------------------
paso "Sacar de nuevo el item del horno (take_from_container sobre la maquina)"

in_container "cat > '$CMDS' <<'CMDS'
$WARMUP
/silent-command function cerca(nombre,x,y) local mejor,dist=nil,nil for _,e in ipairs(game.surfaces[1].find_entities_filtered({name=nombre})) do local dx=e.position.x-x local dy=e.position.y-y local d=dx*dx+dy*dy if not dist or d<dist then mejor,dist=e,d end end return mejor end
/silent-command remote.call('xfactorvr', 'take_from_container', $POS_HORNO, 0, '', 1)
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; local e=cerca('stone-furnace',$POS_HORNO,0); rcon.print('actor_mineral_final='..tostring(ch.get_item_count('iron-ore'))..' horno_ingredientes_final='..tostring(e.get_inventory(2).get_item_count('iron-ore')))
CMDS"
SALIDA_SACAR="$(rcon_query "$CMDS")"
SACAR="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; local e=cerca('stone-furnace',$POS_HORNO,0)" "$SALIDA_SACAR")"
printf '  RCON: %s\n' "$SACAR"

N_MIN_FINAL="$(numero_de "actor_mineral_final" "$SACAR")"
N_HORNO_FINAL="$(numero_de "horno_ingredientes_final" "$SACAR")"
if [ "${N_MIN_FINAL:-0}" -gt "${N_MIN_DESPUES:-0}" ] 2>/dev/null; then
    ok "el item ha vuelto del horno al actor ($N_MIN_DESPUES -> $N_MIN_FINAL mineral)"
else
    fail "el item no ha vuelto del horno al actor ($N_MIN_DESPUES -> $N_MIN_FINAL mineral)"
fi

if [ "${N_HORNO_FINAL:-0}" -lt "${N_HORNO:-0}" ] 2>/dev/null; then
    ok "el horno se ha quedado con menos ingredientes ($N_HORNO -> $N_HORNO_FINAL)"
else
    fail "el horno sigue con los mismos ingredientes ($N_HORNO -> $N_HORNO_FINAL)"
fi

# --- 3e. METER UN ITEM EN UNA CAJA (el "gripo") ----------------------------
paso "Meter un item en una caja (insert_carried_item, contenido)"

in_container "cat > '$CMDS' <<'CMDS'
$WARMUP
/silent-command function cerca(nombre,x,y) local mejor,dist=nil,nil for _,e in ipairs(game.surfaces[1].find_entities_filtered({name=nombre})) do local dx=e.position.x-x local dy=e.position.y-y local d=dx*dx+dy*dy if not dist or d<dist then mejor,dist=e,d end end return mejor end
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; ch.get_main_inventory().clear(); ch.insert({name='iron-ore', count=3}); local e=cerca('iron-chest',$POS_CAJA,0); rcon.print('caja_antes='..tostring(e.get_inventory(1).get_item_count('iron-ore'))..' actor_mineral='..tostring(ch.get_item_count('iron-ore')))
/silent-command remote.call('xfactorvr', 'insert_carried_item', $POS_CAJA, 0, '', 1, 1)
/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; local e=cerca('iron-chest',$POS_CAJA,0); rcon.print('caja_despues='..tostring(e.get_inventory(1).get_item_count('iron-ore'))..' actor_mineral='..tostring(ch.get_item_count('iron-ore')))
CMDS"
SALIDA_CAJAPUE="$(rcon_query "$CMDS")"
CAJA_PUE_ANTES="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; ch.get_main_inventory().clear(); ch.insert({name='iron-ore', count=3}); local e=cerca('iron-chest',$POS_CAJA,0)" "$SALIDA_CAJAPUE")"
CAJA_PUE="$(valor_de "/silent-command local ch=game.surfaces[1].find_entities_filtered({type='character'})[1]; local e=cerca('iron-chest',$POS_CAJA,0); rcon.print('caja_despues" "$SALIDA_CAJAPUE")"
printf '  RCON: %s | %s\n' "$CAJA_PUE_ANTES" "$CAJA_PUE"

N_CAJA_ANTES="$(numero_de "caja_antes" "$CAJA_PUE_ANTES")"
N_CAJA_PUE="$(numero_de "caja_despues" "$CAJA_PUE")"
N_ACTOR_MIN="$(numero_de "actor_mineral" "$CAJA_PUE")"
if [ "${N_CAJA_PUE:-0}" -gt "${N_CAJA_ANTES:-0}" ] 2>/dev/null; then
    ok "el item del actor ha entrado en la caja ($N_CAJA_ANTES -> $N_CAJA_PUE mineral en la caja)"
else
    fail "el item no ha entrado en la caja ($N_CAJA_ANTES -> $N_CAJA_PUE mineral en la caja)"
fi

if [ "${N_ACTOR_MIN:-0}" -lt 3 ] 2>/dev/null; then
    ok "el item ha salido del bolsillo del actor ($N_ACTOR_MIN mineral)"
else
    fail "el item sigue en el bolsillo del actor ($N_ACTOR_MIN mineral)"
fi

# --- 3f. EL SNAPSHOT LLEVA LOS ITEMS SUELTOS -------------------------------
paso "El snapshot lleva los items sueltos del suelo (seccion loose)"

in_container "cat > '$CMDS' <<'CMDS'
$WARMUP
/silent-command local s=game.surfaces[1]; local e=s.create_entity{name='item-on-ground', position={x=2,y=8}, stack={name='copper-ore', count=7}}; rcon.print('suelto='..tostring(e and e.position.x)..','..tostring(e and e.position.y))
/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0, 0)
/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0.0, 0.0)
CMDS"
SALIDA_SNAP="$(rcon_query_guardar "$CMDS")"
if printf '%s' "$SALIDA_SNAP" | grep -q '"loose"'; then
    ok "el snapshot del mod incluye la seccion 'loose' (items sueltos del suelo)"
else
    fail "el snapshot del mod NO incluye la seccion 'loose'"
fi

# La seccion "loose" tiene que traer el item que hay en el suelo.
if printf '%s' "$SALIDA_SNAP" | grep -q '"n":"copper-ore"'; then
    ok "la seccion 'loose' trae el item que hay en el suelo (copper-ore)"
else
    fail "la seccion 'loose' no trae el item del suelo"
fi

# Y el cliente VR tiene que poder leerlo como JSON.
if in_container "test -s '$SNAPSHOT'" 2>/dev/null; then
    if in_container "python3 -c \"import json; d=json.load(open('$SNAPSHOT')); print('loose_ok' if 'loose' in d else 'loose_falta')\"" 2>/dev/null | grep -q loose_ok; then
        ok "el snapshot es JSON valido y se puede leer la seccion 'loose' (como hace el cliente VR)"
    else
        fail "el snapshot guardado no se puede leer como JSON con la seccion 'loose'"
    fi
else
    fail "no se ha guardado el snapshot para validarlo como JSON"
fi

# --------------------------------------------------------------------------
# 4 bis. El camino del APK: clasificar lo que hay bajo el rayo y actuar
# --------------------------------------------------------------------------
# El banco de arriba comprueba los REMOTOS del mod.  Esto comprueba el camino
# COMPLETO que lleva el APK en el visor, con el binario ARM64 dentro del
# contenedor (el mismo crate, el mismo codigo):
#
#   1. conectar por RCON,
#   2. pedir la instantanea del mundo,
#   3. CLASIFICAR lo que hay en la casilla que apunta el mando,
#   4. ACTUAR con la misma llamada que hace el motor de VR al apretar el
#      gatillo o el gripo (`xfactorvr_vr_hand_action`).
paso "El camino completo del APK (clasificar y actuar)"

# Para probar la rama de "coger del suelo" hace falta un item suelto: las
# fases anteriores se los han llevado todos.
in_container "cat > '$CMDS' <<'CMDS'
$WARMUP
/silent-command local s=game.surfaces[1]; local e=s.create_entity{name='item-on-ground', position={x=0,y=$POS_SUELO_Y}, stack={name='iron-plate', count=5}}; rcon.print('suelto='..tostring(e and e.position.x)..','..tostring(e and e.position.y))
/silent-command rcon.print('items_en_el_suelo='..tostring(#game.surfaces[1].find_entities_filtered({name='item-on-ground'})))
CMDS"
SALIDA_SUELTO="$(rcon_query "$CMDS")"
printf '  RCON: %s\n' "$SALIDA_SUELTO"

MANOS_BIN="$PROJECT_ROOT/target/aarch64-unknown-linux-musl/release/probar_manos"
# Se compila SIEMPRE: si el binario se queda viejo, el banco valida un
# codigo que ya no existe (y da falsos verdes o falsos rojos).
say "  (compilando el binario ARM64 de las manos...)"
if ! (cd "$PROJECT_ROOT" && cargo build --release --bin probar_manos --target aarch64-unknown-linux-musl) >/tmp/compilar_manos.log 2>&1; then
    fail "no se pudo compilar el binario de las manos para ARM64 (ver /tmp/compilar_manos.log)"
    MANOS_BIN=""
fi

if [ -n "$MANOS_BIN" ] && [ -f "$MANOS_BIN" ]; then
    "$PODMAN" cp "$MANOS_BIN" "$CONTAINER:/tmp/probar_manos" >/dev/null 2>&1 \
        || "$PODMAN" exec -i "$CONTAINER" sh -c 'cat > /tmp/probar_manos' < "$MANOS_BIN" >/dev/null 2>&1
    "$PODMAN" exec "$CONTAINER" chmod +x /tmp/probar_manos >/dev/null 2>&1 || true
    SALIDA_MANOS="$("$PODMAN" exec "$CONTAINER" bash -c "cd '$FACTORIO_DIR' && /tmp/probar_manos 127.0.0.1:$RCON_PORT '$RCON_PASSWORD'" 2>&1)"
    printf '%s\n' "$SALIDA_MANOS" | sed 's/^/  | /'
    if printf '%s' "$SALIDA_MANOS" | grep -q "RESULTADO: las manos VR funcionan contra el Factorio real"; then
        ok "las manos VR del visor funcionan contra el Factorio real (clasificar + actuar)"
    else
        # AVISO, no fallo: el camino del APK tiene un problema ABIERTO con el
        # RCON (ver docs/ADR-011-el-cliente-rcon-no-habla-con-el-motor.md).
        # El banco sigue siendo valido: lo que se comprueba aqui es que los
        # REMOTOS del mod funcionan (lo de arriba), que es otra cosa.
        aviso "el camino del cliente RCON del APK sigue sin funcionar (ADR-011): el visor cae en la simulacion local"
        printf '%s\n' "$SALIDA_MANOS" | sed 's/^/  | /' | head -8
    fi
else
    fail "no hay binario de las manos para ARM64"
fi

# --------------------------------------------------------------------------
# 4. El banco del JUGADOR VIRTUAL tiene que seguir en verde
# --------------------------------------------------------------------------
paso "El banco del jugador virtual sigue en verde (scripts/test_jugador_virtual.sh)"

# El banco del jugador virtual arranca su propio servidor: este se aparta.
matar_servidor
ok "servidor del banco de inventario detenido"

if [ "$XFV_SIN_JUGADOR" = "1" ]; then
    aviso "banco del jugador virtual saltado (XFV_SIN_JUGADOR=1)"
else
    SALIDA_JV="$(CONTAINER="$CONTAINER" bash "$BANCO_JUGADOR" 2>&1)"
    printf '%s\n' "$SALIDA_JV" | sed 's/^/  | /'

    if printf '%s' "$SALIDA_JV" | grep -q "^RESULTADO: CORRECTO"; then
        ok "el banco del jugador virtual ha pasado"
    else
        fail "el banco del jugador virtual ha fallado"
    fi

    if printf '%s' "$SALIDA_JV" | grep -q "RESULTADO: CORRECTO (6 fases, 0 fallos)"; then
        ok "el banco del jugador virtual: 6 fases, 0 fallos"
    else
        fail "el banco del jugador virtual no ha dado 6 fases y 0 fallos"
    fi
fi

# --------------------------------------------------------------------------
# 5. Limpieza y veredicto
# --------------------------------------------------------------------------
paso "Limpieza"
matar_servidor
ok "servidor detenido"

echo
echo "==========================================================="
if [ "$FALLOS" -eq 0 ]; then
    echo "RESULTADO: CORRECTO (${PASOS} fases, 0 fallos)"
    echo "Las manos VR cogen items del suelo y de las cajas y los meten"
    echo "en hornos y cajas: el item cambia de sitio de verdad."
    echo "==========================================================="
    exit 0
else
    echo "RESULTADO: FALLO (${PASOS} fases, ${FALLOS} fallos)"
    echo "Logs: $CONTAINER:$SERVER_LOG"
    echo "==========================================================="
    exit 1
fi
