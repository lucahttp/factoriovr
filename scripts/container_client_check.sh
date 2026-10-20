#!/bin/bash
set -uo pipefail

# container_client_check.sh — Valida el CLIENTE GRÁFICO de Factorio ARM64 con Zink.
#
# El cliente Linux de Factorio pide OpenGL 3.3 core. En Android no hay OpenGL
# de escritorio, pero Zink (Mesa) da OpenGL de escritorio sobre Vulkan, asi que
# el cliente puede correr sobre el driver Vulkan de Adreno.
#
# En el contenedor no hay GPU, asi que se usa lavapipe (Vulkan por software):
# si el cliente arranca y abre RCON aqui, la cadena completa esta montada y lo
# unico que falta por validar es el driver real del visor.
#
# Uso: bash scripts/container_client_check.sh

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-/opt/podman/bin/podman}"
IMAGE="docker.io/library/ubuntu:24.04"
CONTAINER="xfactorvr-client"
CLIENT_SRC="$PROJECT_ROOT/factorio_client_arm64/factorio"
WORK="/tmp/xfv-client"

echo "=== 1. Preparando el contenedor con Mesa (Zink) y el cliente ==="
"$PODMAN" rm -f "$CONTAINER" >/dev/null 2>&1 || true
"$PODMAN" create --name "$CONTAINER" --platform linux/arm64 "$IMAGE" sleep infinity >/dev/null
"$PODMAN" start "$CONTAINER" >/dev/null

echo "    instalando mesa + sdl2 + vulkan (lavapipe)..."
"$PODMAN" exec "$CONTAINER" bash -c '
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq --no-install-recommends \
    mesa-utils mesa-vulkan-drivers libgl1-mesa-dri libglx-mesa0 libegl1 \
    libvulkan1 libsdl2-2.0-0 xz-utils ca-certificates >/dev/null 2>&1
echo "    paquetes ok"
' || { echo "FALLO al instalar los paquetes"; "$PODMAN" logs "$CONTAINER" | tail -20; exit 1; }

echo "=== 2. Comprobando que Mesa trae Zink y lavapipe ==="
"$PODMAN" exec "$CONTAINER" bash -c '
echo "--- drivers DRI ---"; ls /usr/lib/aarch64-linux-gnu/dri/ | grep -iE "zink|swrast" || echo "NO HAY ZINK"
echo "--- ICD de Vulkan ---"; ls /usr/share/vulkan/icd.d/ || echo "NO HAY ICD"
echo "--- version de Mesa ---"; dpkg -l | grep -E "mesa" | awk "{print \$2, \$3}" | head -5
'

echo "=== 3. Copiando el cliente grafico ARM64 ==="
"$PODMAN" exec "$CONTAINER" mkdir -p /factorio
tar -C "$PROJECT_ROOT/factorio_client_arm64" -cf - factorio | \
    "$PODMAN" exec -i "$CONTAINER" tar -C / -xf -
"$PODMAN" exec "$CONTAINER" ls -la /factorio/bin/arm64/ | head -3

echo "=== 4. Probando Zink: OpenGL sobre Vulkan ==="
"$PODMAN" exec "$CONTAINER" bash -c '
export LIBGL_DRIVERS_PATH=/usr/lib/aarch64-linux-gnu/dri
export MESA_LOADER_DRIVER_OVERRIDE=zink
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.aarch64.json
export ZINK_DESCRIPTORS=auto
echo "--- vulkaninfo (lavapipe) ---"
vulkaninfo --summary 2>&1 | grep -E "deviceName|apiVersion|driverName" | head -6
echo "--- glxinfo con Zink ---"
glxinfo -B 2>&1 | grep -E "OpenGL renderer|OpenGL version|OpenGL core profile version|failed|cannot" | head -8
' || true

echo "=== 5. Arrancando el cliente grafico con Zink y SDL offscreen ==="
"$PODMAN" exec -d "$CONTAINER" bash -c '
export LIBGL_DRIVERS_PATH=/usr/lib/aarch64-linux-gnu/dri
export MESA_LOADER_DRIVER_OVERRIDE=zink
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.aarch64.json
export ZINK_DESCRIPTORS=auto
export SDL_VIDEODRIVER=offscreen
export LIBGL_ALWAYS_SOFTWARE=0
cd /factorio && mkdir -p logs saves config
cat > config/config-path.cfg <<CFG
config-path=/factorio/config
use-system-read-write-data-directories=false
CFG
./bin/arm64/factorio --version 2>&1 | head -5
' || true
sleep 3
"$PODMAN" exec "$CONTAINER" bash -c '
export LIBGL_DRIVERS_PATH=/usr/lib/aarch64-linux-gnu/dri
export MESA_LOADER_DRIVER_OVERRIDE=zink
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.aarch64.json
export ZINK_DESCRIPTORS=auto
export SDL_VIDEODRIVER=offscreen
cd /factorio
./bin/arm64/factorio --create /factorio/saves/client_test.zip 2>&1 | tail -25
' || true

echo "=== 6. Estado final ==="
"$PODMAN" exec "$CONTAINER" bash -c 'ls -la /factorio/saves/ 2>/dev/null; ls /factorio/*.log 2>/dev/null' || true

echo "=== Contenedor listo para depurar: $PODMAN exec -it $CONTAINER bash ==="
echo "=== Logs del cliente en: /factorio/logs/ ==="
