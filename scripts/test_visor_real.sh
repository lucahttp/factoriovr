#!/bin/bash
# test_visor_real.sh — Banco del VISOR contra el motor real (sin dispositivo).
#
# El banco del inventario (scripts/test_inventario_vr.sh) comprueba los
# REMOTOS del mod por sondeo Python. Este comprueba el CAMINO COMPLETO que
# lleva el APK en el visor, con el binario ARM64 del mismo crate dentro del
# contenedor:
#
#   1. el motor arranca headless con el mod puente y el RCON escuchando,
#   2. el snapshot que manda el mod trae las entidades con el TIPO CANONICO
#      (campo "c"), que es lo que permite al cliente saber que son,
#   3. el cliente Rust (el del visor) mete esas entidades en el DIORAMA:
#      no vale la simulacion local de ejemplo (11 entidades de fabrica
#      inventada),
#   4. las manos VR clasifican y actuan sobre el juego real,
#   5. el poll NO martillea al motor: el numero de conexiones TCP nuevas en un
#      intervalo tiene que ser pequeño, no cientos.
#
# La partida es la de la CAMPANA (base/xfv-virtual), que es la que se juega:
# ahi los arboles son "tree-03/04/05/07", prototipos que NO estan en la tabla
# de tipos del cliente, y por eso comprueba el tipo canonico.
#
# Codigos de salida:
#   0  todo correcto
#   1  algo falla
#   2  el entorno no permite validarlo (sin podman, sin contenedor, ...)
#
# Uso: bash scripts/test_visor_real.sh
#
# Variables de entorno:
#   PODMAN          binario de podman (por defecto: podman)
#   CONTAINER       contenedor (por defecto: xfactorvr-client)
#   FACTIO_DIR      raiz del servidor dentro del contenedor (/factorio)
#   ESCENARIO       escenario (base/xfv-virtual)
#   PUERTO          puerto del juego (34203)
#   RCON_PORT       puerto del RCON (27022)
#   RCON_PASSWORD   contrasena del RCON (xfactorvr)
#   XFV_SIN_REGRESION 1 = no ejecutar la regresion del banco del inventario

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-podman}"
CONTAINER="${CONTAINER:-xfactorvr-client}"
FACTORIO_DIR="${FACTORIO_DIR:-/factorio}"
ESCENARIO="${ESCENARIO:-base/xfv-virtual}"
PUERTO="${PUERTO:-34203}"
RCON_PORT="${RCON_PORT:-27022}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
XFV_SIN_REGRESION="${XFV_SIN_REGRESION:-0}"

MOD_PUENTE="$PROJECT_ROOT/mods/xfactorvr_bridge"
EXTRACTOR="$PROJECT_ROOT/scripts/extraer_snapshot.py"
COMANDOS="$PROJECT_ROOT/scripts/rcon_comandos.py"
BANCO_INVENTARIO="$PROJECT_ROOT/scripts/test_inventario_vr.sh"

SERVER_LOG="$FACTORIO_DIR/visor_real_server.log"
SERVER_SETTINGS="$FACTORIO_DIR/config/server-settings-visor-real.json"
SNAPSHOT="$FACTORIO_DIR/tmp/visor_real_snapshot.json"
MANOS_BIN="$PROJECT_ROOT/target/aarch64-unknown-linux-musl/release/probar_manos"

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

# Por PID y no con pkill -f: el patron "rcon-port ..." tambien aparece en la
# linea de comandos del propio shell que lanza el servidor.
#
# OJO: no se puede mirar el fichero .lock para saber si el motor esta vivo:
# Factorio lo deja en disco y el siguiente arranque se queja de que "ya hay
# otra instancia" aunque el proceso este muerto.  La unica fuente de verdad
# son los procesos.
matar_servidor() {
    in_container 'ps -eo pid,args | awk "\$2 ~ /factorio\$/ {print \$1}" | xargs -r kill 2>/dev/null; true'
    for i in $(seq 1 30); do
        if ! in_container "ps -eo args | grep -q '[f]actorio --start-server'"; then
            return 0
        fi
        sleep 1
    done
    return 1
}

# --------------------------------------------------------------------------
# 0. Preflight
# --------------------------------------------------------------------------
paso "Preflight del entorno"

command -v "$PODMAN" >/dev/null 2>&1 || sin_entorno "no esta '$PODMAN' en el PATH"
"$PODMAN" ps --format '{{.Names}}' 2>/dev/null | grep -qx "$CONTAINER" \
    || sin_entorno "el contenedor '$CONTAINER' no esta arrancado"

for f in "$EXTRACTOR" "$COMANDOS"; do
    [ -f "$f" ] || sin_entorno "falta $f"
done
[ -d "$MOD_PUENTE" ] || sin_entorno "falta mods/xfactorvr_bridge"

in_container "test -e '$FACTORIO_DIR/bin/arm64/factorio'" || sin_entorno "no esta el motor en el contenedor"
in_container "test -f '$FACTORIO_DIR/data/${ESCENARIO%%/*}/scenarios/${ESCENARIO#*/}/control.lua'" \
    || sin_entorno "el escenario '$ESCENARIO' no esta en el contenedor"
in_container "command -v python3 >/dev/null" || sin_entorno "el contenedor no tiene python3"
ok "contenedor '$CONTAINER' con el motor, el escenario '$ESCENARIO' y python3"

# --------------------------------------------------------------------------
# 1. Mod puente + servidor headless con RCON
# --------------------------------------------------------------------------
paso "Mod puente y arranque del servidor headless"

in_container "mkdir -p '$FACTORIO_DIR/tmp' '$FACTORIO_DIR/config'"
"$PODMAN" cp "$MOD_PUENTE" "$CONTAINER:$FACTORIO_DIR/mods/" >/dev/null
in_container "cat > '$FACTORIO_DIR/mods/mod-list.json' <<'JSON'
{
  \"mods\": [
    { \"name\": \"base\", \"enabled\": true },
    { \"name\": \"xfactorvr_bridge\", \"enabled\": true }
  ]
}
JSON"

# auto_pause=false: sin jugadores el juego se congela y el personaje virtual
# no puede recoger ni meter nada.
in_container "cat > '$SERVER_SETTINGS' <<'JSON'
{
  \"name\": \"XFactorVR-Visor-Real\",
  \"description\": \"Banco del visor contra el motor real\",
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

matar_servidor
in_container "cd '$FACTORIO_DIR' && (sleep 1800 | ./bin/arm64/factorio \
    --start-server-load-scenario '$ESCENARIO' \
    --port $PUERTO --rcon-port $RCON_PORT --rcon-password '$RCON_PASSWORD' \
    --server-settings '$SERVER_SETTINGS') > '$SERVER_LOG' 2>&1 &"

RCON_LISTO=0
for i in $(seq 1 90); do
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
    sin_entorno "sin servidor no se puede validar el visor"
fi

# --------------------------------------------------------------------------
# 2. El snapshot del mod trae el TIPO CANONICO
# --------------------------------------------------------------------------
paso "El snapshot del mod trae el tipo canonico de cada entidad"

"$PODMAN" cp "$EXTRACTOR" "$CONTAINER:/tmp/extraer_snapshot.py" >/dev/null
"$PODMAN" cp "$COMANDOS" "$CONTAINER:/tmp/rcon_comandos.py" >/dev/null
SALIDA_SNAP="$("$PODMAN" exec "$CONTAINER" python3 /tmp/extraer_snapshot.py \
    "127.0.0.1:$RCON_PORT" "$RCON_PASSWORD" "$SNAPSHOT" 2>&1)"
printf '  %s\n' "$SALIDA_SNAP" | head -2

if in_container "test -s '$SNAPSHOT'"; then
    "$PODMAN" cp "$CONTAINER:$SNAPSHOT" "$PROJECT_ROOT/build/snapshot_visor_real.json" >/dev/null
    ok "snapshot guardado ($(wc -c < "$PROJECT_ROOT/build/snapshot_visor_real.json") bytes)"
else
    fail "no se pudo sacar el snapshot por RCON"
    sin_entorno "sin snapshot no se puede validar el visor"
fi

# El recuento lo hace Python dentro del contenedor: el snapshot trae dos bytes
# null al final que no son JSON.
CONTEO="$(in_container "python3 - <<'PY'
import json
raw = open('$SNAPSHOT', 'rb').read()
if raw.endswith(b'\x00\x00'):
    raw = raw[:-2]
d = json.loads(raw.decode('utf-8'))
ents = d.get('entities', [])
con_c = [e for e in ents if e.get('c')]
arboles = [e for e in ents if e.get('c') == 'tree']
protos = sorted({e.get('t') for e in ents})
print('entidades=%d con_c=%d arboles=%d prototipos=%s' % (len(ents), len(con_c), len(arboles), ','.join(str(p) for p in protos[:8])))
PY")"
printf '  %s\n' "$CONTEO"

ENTIDADES="$(printf '%s' "$CONTEO" | sed -n 's/.*entidades=\([0-9]*\).*/\1/p')"
CON_C="$(printf '%s' "$CONTEO" | sed -n 's/.*con_c=\([0-9]*\).*/\1/p')"
ARBOLES="$(printf '%s' "$CONTEO" | sed -n 's/.*arboles=\([0-9]*\).*/\1/p')"

if [ "${ENTIDADES:-0}" -gt 0 ]; then
    ok "el snapshot trae $ENTIDADES entidades del juego real"
else
    fail "el snapshot viene vacio"
fi

if [ "${CON_C:-0}" -eq "${ENTIDADES:-0}" ] && [ "${ENTIDADES:-0}" -gt 0 ]; then
    ok "todas las entidades traen el tipo canonico (campo \"c\")"
elif [ "${CON_C:-0}" -gt 0 ]; then
    aviso "solo $CON_C de $ENTIDADES entidades traen el tipo canonico"
else
    fail "ninguna entidad trae el tipo canonico: el cliente no puede saber que son"
fi

if [ "${ARBOLES:-0}" -gt 0 ]; then
    ok "$ARBOLES arboles con prototipo no canonico (el caso que se descartaba)"
else
    aviso "la partida no tiene arboles: el caso del prototipo no canonico no se puede comprobar"
fi

# --------------------------------------------------------------------------
# 3. El diorama del cliente trae el juego real, no la simulacion local
# --------------------------------------------------------------------------
paso "El diorama del cliente trae el juego real"

# El binario tiene que ser MAS NUEVO que el codigo: si no, el banco valida un
# binario viejo (y asi se pasaron por alto varios arreglos).
if [ -f "$MANOS_BIN" ]; then
    if [ "$PROJECT_ROOT/src/bin/probar_manos.rs" -nt "$MANOS_BIN" ] \
       || [ "$PROJECT_ROOT/src/game_bridge.rs" -nt "$MANOS_BIN" ]; then
        aviso "el binario de las manos esta viejo: recompilalo con"
        aviso "  cargo build --release --bin probar_manos --target aarch64-unknown-linux-musl"
    fi
else
    aviso "no hay binario de las manos para ARM64: se compila con"
    aviso "  cargo build --release --bin probar_manos --target aarch64-unknown-linux-musl"
fi

if [ ! -f "$MANOS_BIN" ]; then
    sin_entorno "sin el binario ARM64 del cliente no se puede validar el diorama"
fi

"$PODMAN" cp "$MANOS_BIN" "$CONTAINER:/tmp/probar_manos" >/dev/null 2>&1 \
    || "$PODMAN" exec -i "$CONTAINER" sh -c 'cat > /tmp/probar_manos' < "$MANOS_BIN" >/dev/null 2>&1
"$PODMAN" exec "$CONTAINER" chmod +x /tmp/probar_manos >/dev/null 2>&1 || true

# El cliente necesita una caja, un horno y un item en el suelo para poder
# clasificar y actuar (es lo que hacen las manos en MR).
SALIDA_OBJ="$("$PODMAN" exec "$CONTAINER" python3 /tmp/rcon_comandos.py \
    "127.0.0.1:$RCON_PORT" "$RCON_PASSWORD" \
    "/silent-command local s=game.surfaces[1]; s.create_entity{name='wooden-chest', position={x=6,y=0}}; s.create_entity{name='stone-furnace', position={x=-6,y=0}}; s.create_entity{name='item-on-ground', position={x=0,y=6}, stack={name='iron-plate', count=5}}; rcon.print('objetos_listos')" \
    2>&1)"
printf '  %s\n' "$SALIDA_OBJ" | head -2

# Numero de conexiones TCP nuevas que abre el motor ANTES de que corra el
# cliente: sirve de referencia para comparar despues.
CONEXIONES_ANTES="$(in_container "grep -c 'New RCON connection' '$SERVER_LOG' 2>/dev/null || echo 0")"

SALIDA_MANOS="$("$PODMAN" exec "$CONTAINER" bash -c "cd '$FACTORIO_DIR' && /tmp/probar_manos 127.0.0.1:$RCON_PORT '$RCON_PASSWORD'" 2>&1)"
printf '%s\n' "$SALIDA_MANOS" | sed 's/^/  | /' | head -25

DIORAMA="$(printf '%s' "$SALIDA_MANOS" | sed -n 's/.*instantanea con \([0-9]*\) entidades.*/\1/p' | head -1)"
DIORAMA="${DIORAMA:-0}"

if [ "$DIORAMA" -gt 11 ]; then
    ok "el diorama tiene $DIORAMA entidades (la simulacion local solo pone 11)"
elif [ "$DIORAMA" -gt 0 ]; then
    fail "el diorama solo tiene $DIORAMA entidades: sigue en la simulacion local"
else
    fail "el diorama esta vacio: el cliente no aplica el snapshot del motor"
fi

if [ "${ENTIDADES:-0}" -gt 0 ] && [ "$DIORAMA" -ge "$((ENTIDADES - 1))" ]; then
    ok "el diorama trae todas las entidades del snapshot ($DIORAMA de $ENTIDADES)"
else
    fail "el diorama trae $DIORAMA entidades y el snapshot trae $ENTIDADES"
fi

if printf '%s' "$SALIDA_MANOS" | grep -q "RESULTADO: las manos VR funcionan contra el Factorio real"; then
    ok "las manos VR clasifican y actuan sobre el juego real"
else
    fail "las manos VR no llegan a actuar sobre el Factorio real"
fi

# --------------------------------------------------------------------------
# 4. El poll no martillea al motor
# --------------------------------------------------------------------------
paso "El poll no martillea al motor"

CONEXIONES_DESPUES="$(in_container "grep -c 'New RCON connection' '$SERVER_LOG' 2>/dev/null || echo 0")"
CONEXIONES_CLIENTE=$((CONEXIONES_DESPUES - CONEXIONES_ANTES))
printf '  conexiones TCP nuevas abiertas por el cliente: %s\n' "$CONEXIONES_CLIENTE"

# El poll corre a 8 Hz. Si reinicia la sesion en cada ciclo, en los pocos
# segundos que tarda el cliente se abren decenas de conexiones.
if [ "$CONEXIONES_CLIENTE" -le 6 ]; then
    ok "el cliente abre pocas conexiones: no reinicia la sesion en cada poll"
else
    fail "el cliente abre $CONEXIONES_CLIENTE conexiones TCP: el poll reinicia la sesion en cada ciclo"
fi

# --------------------------------------------------------------------------
# 5. Regresion: el banco del inventario tiene que seguir en verde
# --------------------------------------------------------------------------
paso "Regresion: el banco del inventario sigue en verde"

matar_servidor

if [ "$XFV_SIN_REGRESION" = "1" ]; then
    aviso "regresion saltada (XFV_SIN_REGRESION=1)"
else
    SALIDA_INV="$(CONTAINER="$CONTAINER" bash "$BANCO_INVENTARIO" 2>&1)"
    printf '%s\n' "$SALIDA_INV" | tail -25 | sed 's/^/  | /'
    if printf '%s' "$SALIDA_INV" | grep -q "^RESULTADO: CORRECTO"; then
        ok "el banco del inventario sigue en verde"
    else
        fail "el banco del inventario ya no pasa"
        printf '%s\n' "$SALIDA_INV" | grep -i "fallo" | sed 's/^/  | /' | head -10
    fi
fi

# --------------------------------------------------------------------------
# Resumen
# --------------------------------------------------------------------------
printf '\n=== RESULTADO ===\n'
printf 'pasos: %d   fallos: %d\n' "$PASOS" "$FALLOS"
if [ "$FALLOS" -eq 0 ]; then
    printf 'RESULTADO: CORRECTO\n'
    exit 0
fi
printf 'RESULTADO: INCORRECTO\n'
exit 1
