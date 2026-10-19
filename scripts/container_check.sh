#!/bin/bash
set -euo pipefail

# container_check.sh — Levanta el servidor headless REAL de Factorio 2.0 en un
# contenedor Linux ARM64 y valida el puente RCON de punta a punta.
#
# Es la validacion que faltaba: el mod se probaba contra un mock, y un mock no
# puede detectar que una llamada a la API no exista en el motor. Asi se
# encontraron llamadas rotas en Factorio 2.0 (game.recipe_prototypes,
# inventory.remove_item, entity.health_max, entity.recipe, entity.rotation).
#
# El contenedor usa la misma arquitectura ARM64 y la misma glibc que el visor
# Pico 4 Ultra, asi que lo que funciona aqui funciona ahi.
#
# Uso: bash scripts/container_check.sh

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-/opt/podman/bin/podman}"
IMAGE="docker.io/library/ubuntu:24.04"
RCON_PORT="${RCON_PORT:-27015}"
RCON_PASSWORD="${RCON_PASSWORD:-xfactorvr}"
CONTAINER="xfactorvr-factorio-check"

cleanup() {
    "$PODMAN" rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "=== Validacion del servidor Factorio 2.0 real en contenedor ARM64 ==="

# La VM de podman puede quedarse sin socket SSH (el gvproxy cae al publicar o
# al borrar contenedores con puertos). Se revive antes de seguir.
if ! "$PODMAN" ps >/dev/null 2>&1; then
    echo "--- 0. La VM de podman no responde; reiniciandola ---"
    "$PODMAN" machine stop podman-machine-default >/dev/null 2>&1 || true
    sleep 3
    "$PODMAN" machine start podman-machine-default >/dev/null 2>&1 || true
    for i in $(seq 1 30); do
        "$PODMAN" ps >/dev/null 2>&1 && break
        sleep 2
    done
    if ! "$PODMAN" ps >/dev/null 2>&1; then
        echo "No se pudo recuperar la VM de podman." >&2
        exit 1
    fi
    echo "    VM lista"
fi

if [ ! -x "$PROJECT_ROOT/factorio_arm64/bin/arm64/factorio" ]; then
    echo "Falta el motor headless en factorio_arm64/bin/arm64/factorio" >&2
    exit 1
fi

# La carpeta del mod debe llamarse <nombre>_<version> de info.json, o Factorio
# no lo carga. Se deriva del propio info.json para que no se desincronice.
MOD_ID=$(python3 -c "
import json
d = json.load(open('$PROJECT_ROOT/mods/xfactorvr_bridge/info.json'))
print(d['name'] + '_' + d['version'])
")
STAGE="$PROJECT_ROOT/build/mod_stage/$MOD_ID"
rm -rf "$PROJECT_ROOT/build/mod_stage"
mkdir -p "$STAGE"
cp "$PROJECT_ROOT/mods/xfactorvr_bridge/control.lua" "$STAGE/"
cp "$PROJECT_ROOT/mods/xfactorvr_bridge/info.json" "$STAGE/"
echo "Mod a validar: $MOD_ID (factorio_version $(python3 -c "
import json
print(json.load(open('$PROJECT_ROOT/mods/xfactorvr_bridge/info.json'))['factorio_version'])
"))"

echo "--- 1. Preparando la imagen $IMAGE ---"
"$PODMAN" pull --platform linux/arm64 "$IMAGE" >/dev/null 2>&1 || true

echo "--- 2. Empaquetando motor y mod para el contenedor ---"
# Se copia un unico .tar en vez de miles de ficheros: por el canal gRPC del
# VM de podman, copiar un arbol grande con miles de ficheros se corta.
STAGE_ROOT="$PROJECT_ROOT/build/container_stage"
rm -rf "$STAGE_ROOT"
mkdir -p "$STAGE_ROOT/mods/$MOD_ID"
cp "$PROJECT_ROOT/mods/xfactorvr_bridge/control.lua" "$STAGE_ROOT/mods/$MOD_ID/"
cp "$PROJECT_ROOT/mods/xfactorvr_bridge/info.json" "$STAGE_ROOT/mods/$MOD_ID/"

# --no-xattrs evita que bsdtar meta las etiquetas extendidas de macOS, que
# GNU tar avisa pero no entiende.
PAYLOAD_TAR="$PROJECT_ROOT/build/container_stage.tar"
COPYFILE_DISABLE=1 tar --no-xattrs -cf "$PAYLOAD_TAR" \
    -C "$PROJECT_ROOT/factorio_arm64" data config-path.cfg bin
COPYFILE_DISABLE=1 tar --no-xattrs -rf "$PAYLOAD_TAR" \
    -C "$STAGE_ROOT" mods
echo "    payload: $(du -h "$PAYLOAD_TAR" | cut -f1)"

echo "--- 3. Arrancando el contenedor con el motor ---"
"$PODMAN" rm -f "$CONTAINER" >/dev/null 2>&1 || true
# Sin -p: publicar el puerto RCON en el host ata el gvproxy de la VM al ciclo de
# vida del contenedor, y al borrarlo el gvproxy cae y se lleva por delante el
# socket SSH de la maquina. El cliente RCON se ejecuta dentro del contenedor.
"$PODMAN" create --name "$CONTAINER" --platform linux/arm64 \
    "$IMAGE" sleep infinity >/dev/null

COPIED=0
for attempt in 1 2 3; do
    if "$PODMAN" cp "$PAYLOAD_TAR" "$CONTAINER:/payload.tar" >/dev/null 2>&1; then
        COPIED=1
        break
    fi
    echo "    reintento de copia ($attempt/3)..."
    sleep 5
done
if [ "$COPIED" -ne 1 ]; then
    echo "No se pudo copiar el motor al contenedor." >&2
    exit 1
fi
"$PODMAN" start "$CONTAINER" >/dev/null

echo "--- 4. Creando la partida y arrancando el servidor ---"
"$PODMAN" exec --env MOD_ID="$MOD_ID" "$CONTAINER" sh -c '
    set -e
    mkdir -p /factorio
    cd /factorio
    tar -xf /payload.tar
    rm -f /payload.tar
    chmod 755 /factorio/bin/arm64/factorio
    cd /factorio
    echo "--- mod instalado ---"
    ls /factorio/mods/$MOD_ID/
    echo "--- creando la partida ---"
    ./bin/arm64/factorio --create xfactorvr_world.zip --mod-directory /factorio/mods || true
    echo "--- partidas ---"
    ls -la /factorio/saves/ || true
' 2>&1 | tail -25

# El servidor se lanza en segundo plano dentro del contenedor.
"$PODMAN" exec -d "$CONTAINER" sh -c '
    cd /factorio
    SAVE=$(ls /factorio/saves/*.zip 2>/dev/null | head -n 1)
    echo "partida: $SAVE"
    exec ./bin/arm64/factorio \
        --start-server "$SAVE" \
        --rcon-port '"$RCON_PORT"' \
        --rcon-password '"$RCON_PASSWORD"' \
        --mod-directory /factorio/mods
'

echo "--- 4. Esperando al puerto RCON ---"
READY=0
for i in $(seq 1 120); do
    if "$PODMAN" exec "$CONTAINER" sh -c \
        "grep -qi ':$(printf '%04X' $RCON_PORT) ' /proc/net/tcp" 2>/dev/null; then
        echo "    RCON escuchando en el puerto $RCON_PORT tras ${i}s"
        READY=1
        break
    fi
    sleep 1
done

echo ""
echo "--- 5. Log del servidor ---"
"$PODMAN" logs "$CONTAINER" 2>&1 | tail -30
echo ""

if [ "$READY" -ne 1 ]; then
    echo "RESULTADO: el servidor no abrio el puerto RCON."
    echo "Log del contenedor:"
    "$PODMAN" exec "$CONTAINER" sh -c 'cat /factorio/logs/*.log 2>/dev/null | tail -30' || true
    exit 1
fi

echo "--- 6. Comprobando el puente RCON con los comandos reales del cliente ---"
# El sondeo corre dentro del contenedor: el puerto no se publica al host.
"$PODMAN" cp "$PROJECT_ROOT/scripts/rcon_probe.py" "$CONTAINER:/rcon_probe.py" >/dev/null 2>&1
"$PODMAN" exec "$CONTAINER" python3 /rcon_probe.py 127.0.0.1 "$RCON_PORT" "$RCON_PASSWORD" \
    "/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0.0, 0.0)" \
    "remote.call('xfactorvr', 'get_world_snapshot', 32, 0.0, 0.0)" \
    "/silent-command remote.call('xfactorvr', 'get_inventory')" \
    "remote.call('xfactorvr', 'craft', 'iron-gear-wheel', 1)" || true

echo ""
echo "--- 7. Log del servidor tras las llamadas ---"
"$PODMAN" exec "$CONTAINER" sh -c 'tail -25 /factorio/logs/*.log 2>/dev/null' || true
