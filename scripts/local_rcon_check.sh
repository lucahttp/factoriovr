#!/bin/bash
set -euo pipefail

# local_rcon_check.sh — Banco de pruebas RCON sin contenedor.
#
# El contenedor ARM64 es la prueba de fidelidad (mismo binario y glibc que el
# visor), pero es lento y la VM de podman se cae con facilidad en este equipo.
# Para responder preguntas del PROTOCOLO RCON (¿vuelve el JSON al cliente? ¿el
# servidor acepta /silent-command?) basta el Factorio de escritorio 2.0.77 que
# ya esta instalado, y el ciclo es de segundos.
#
# Uso: bash scripts/local_rcon_check.sh
#
# Variables de entorno:
#   FACTIO_BIN   ruta al binario de Factorio de escritorio
#   RCON_PORT    puerto RCON (por defecto 27015)
#   RCON_PASSWORD  contrasena RCON (por defecto xfactorvr)

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FACTORIO_HOME="/Volumes/X10Pro/SteamLibrary/steamapps/common/Factorio"
FACTIO_BIN="${FACTIO_BIN:-$FACTORIO_HOME/factorio.app/Contents/MacOS/factorio}"
RCON_PORT="${RCON_PORT:-27015}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
WORK="$PROJECT_ROOT/tmp/local_rcon"

if [ ! -x "$FACTIO_BIN" ]; then
    echo "No se encuentra el binario de Factorio de escritorio en $FACTIO_BIN" >&2
    exit 1
fi

# El mod declara factorio_version 2.1 (el motor ARM64 del visor es 2.1.22).
# El cliente de escritorio puede ser de otra version, asi que se genera una
# copia del mod con la version del cliente que se va a probar.
MOD_ID=$(python3 -c "
import json
d = json.load(open('$PROJECT_ROOT/mods/xfactorvr_bridge/info.json'))
print(d['name'] + '_' + d['version'])
")
CLIENT_DATA="$FACTORIO_HOME/factorio.app/Contents/data"

rm -rf "$WORK"
mkdir -p "$WORK/mods/$MOD_ID" "$WORK/saves"
cp "$PROJECT_ROOT/mods/xfactorvr_bridge/control.lua" "$WORK/mods/$MOD_ID/"
python3 - "$PROJECT_ROOT/mods/xfactorvr_bridge/info.json" "$WORK/mods/$MOD_ID/info.json" "$CLIENT_DATA" <<'PY'
import json, sys, os
mod_info, out_info, client_data = sys.argv[1], sys.argv[2], sys.argv[3]
d = json.load(open(mod_info))
try:
    ver = json.load(open(os.path.join(client_data, "base", "info.json")))["version"]
    major_minor = ".".join(ver.split(".")[:2])
    if d.get("factorio_version") != major_minor:
        d["factorio_version"] = major_minor
        print(f"    mod ajustado a factorio_version {major_minor} (cliente {ver})")
except Exception as exc:
    print(f"    aviso: no se pudo leer la version del cliente ({exc})")
json.dump(d, open(out_info, "w"), indent=2)
PY

echo "=== Creando partida (Factorio de escritorio 2.0.77) ==="
"$FACTIO_BIN" --create "$WORK/saves/rcon_test.zip" --mod-directory "$WORK/mods" 2>&1 | tail -5

echo ""
echo "=== Arrancando servidor con RCON en el puerto $RCON_PORT ==="
# auto_pause false: sin jugadores conectados el juego se queda congelado y no
# hay simulacion que mirar. La clave es auto_pause (guion bajo), no auto-pause.
cp "$PROJECT_ROOT/scripts/server-settings.json" "$WORK/server-settings.json"
"$FACTIO_BIN" --start-server "$WORK/saves/rcon_test.zip" \
    --rcon-port "$RCON_PORT" \
    --rcon-password "$RCON_PASSWORD" \
    --server-settings "$WORK/server-settings.json" \
    --mod-directory "$WORK/mods" > "$WORK/server.log" 2>&1 &
SERVER_PID=$!

cleanup() {
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT

READY=0
for i in $(seq 1 90); do
    if python3 -c "
import socket,sys
s=socket.socket()
s.settimeout(1)
sys.exit(0 if s.connect_ex(('127.0.0.1',$RCON_PORT))==0 else 1)
" 2>/dev/null; then
        echo "    RCON escuchando tras ${i}s"
        READY=1
        break
    fi
    sleep 1
done

if [ "$READY" -ne 1 ]; then
    echo "El servidor no abrio el puerto RCON. Log:"
    tail -30 "$WORK/server.log"
    exit 1
fi

echo ""
echo "=== Sondeando el protocolo RCON ==="
# Los comandos van en un fichero para no pelear con las comillas de Lua.
cat > "$WORK/commands.txt" <<'CMDS'
/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0.0, 0.0)
/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0.0, 0.0)
/silent-command remote.call('xfactorvr', 'get_inventory')
/silent-command remote.call('xfactorvr', 'craft', 'iron-gear-wheel', 1)
/silent-command rcon.print('DIAG_total=' .. tostring(#game.surfaces[1].find_entities({{-32,-32},{32,32}})))
/silent-command rcon.print('DIAG_filtradas=' .. tostring(#game.surfaces[1].find_entities_filtered({{-32,-32},{32,32}}, {type="resource"})))
/silent-command rcon.print('DIAG_arboles=' .. tostring(#game.surfaces[1].find_entities_filtered({{-32,-32},{32,32}}, {type="tree"})))
/silent-command rcon.print('DIAG_todo=' .. tostring(#game.surfaces[1].find_entities_filtered({{-32,-32},{32,32}}, {name="iron-ore"})))
/silent-command rcon.print('DIAG_tick=' .. tostring(game.tick))
/silent-command rcon.print('DIAG_pausado=' .. tostring(game.tick_paused))
CMDS
python3 "$PROJECT_ROOT/scripts/rcon_probe.py" --file "$WORK/commands.txt" "$RCON_PORT" "$RCON_PASSWORD" || true

echo ""
echo "=== Log del servidor (ultimas lineas) ==="
tail -20 "$WORK/server.log"
