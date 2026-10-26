#!/usr/bin/env bash
set -uo pipefail

# diag_frames.sh — Por que el visor no ve la imagen del motor.
#
# Sintoma: el motor entra en juego y renderiza (el inyector cuenta miles de
# frames) pero al visor solo le llegan 3: el resto los descarta por "vacios".
# O sea: el motor dibuja en negro.
#
# La prueba: se volca un frame del motor a PNG en el visor (el inyector tiene
# XFACTORVR_3D_CAPTURA) y se mira.  Ademas se prueba el cliente con lavapipe
# (Vulkan por software) por si el problema es el driver Turnip.
#
# Uso: bash scripts/diag_frames.sh

ADB="${ADB:-$HOME/Library/Android/platform-tools/adb}"
REMOTE="/data/local/tmp"
MESA="$REMOTE/mesa/usr/lib/aarch64-linux-gnu"

if ! $ADB devices 2>/dev/null | grep -q "device$"; then
    echo "NO VALIDABLE: no hay visor conectado por adb"
    exit 2
fi

matar_cliente() {
    $ADB shell 'pat="factorio_cliente/config"; for p in /proc/[0-9]*; do [ "$p" = "/proc/$$" ] && continue; if grep -qa "$pat" $p/cmdline 2>/dev/null; then kill ${p#/proc/} 2>/dev/null; fi; done; sleep 2; true' >/dev/null 2>&1
}

traer_png() {
    local remoto="$1" local="$2"
    $ADB exec-out "toybox base64 $remoto" 2>/dev/null > /tmp/_f.b64
    base64 -D -i /tmp/_f.b64 -o "$local" 2>/dev/null || \
        openssl base64 -d -in /tmp/_f.b64 -out "$local" 2>/dev/null
    ls -la "$local" 2>/dev/null
}

echo "=== 1. Cliente con TURNIP (el de siempre) ==="
matar_cliente
$ADB shell "cd $REMOTE && nohup env XFACTORVR_USE_TURNIP=1 SERVER_ADDR=127.0.0.1 GAME_DIR=$REMOTE/factorio XFACTORVR_3D_CAPTURA=/tmp/frame_turnip.png sh $REMOTE/start_factorio_client.sh > $REMOTE/cli_turnip.log 2>&1 &" >/dev/null 2>&1
sleep 70
$ADB shell "grep -a 'captura\|frame completo' $REMOTE/cli_turnip.log | tail -2" 2>&1 | sed 's/^/  /'
traer_png /tmp/frame_turnip.png /tmp/frame_turnip.png

echo ""
echo "=== 2. Cliente con LAVAPIPE (Vulkan por software) ==="
matar_cliente
$ADB shell "cd $REMOTE && nohup env SERVER_ADDR=127.0.0.1 GAME_DIR=$REMOTE/factorio XFACTORVR_PROXY_PUERTO=8899 XFACTORVR_3D_CAPTURA=/tmp/frame_lvp.png sh $REMOTE/start_factorio_client.sh > $REMOTE/cli_lvp.log 2>&1 &" >/dev/null 2>&1
sleep 70
$ADB shell "grep -a 'captura\|frame completo' $REMOTE/cli_lvp.log | tail -2" 2>&1 | sed 's/^/  /'
traer_png /tmp/frame_lvp.png /tmp/frame_lvp.png

echo ""
echo "Mira los dos PNG: si el de lavapipe tiene el juego y el de turnip esta"
echo "negro, el problema es el driver (y la pista es el FBO del mundo)."
