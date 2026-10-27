#!/usr/bin/env bash
set -uo pipefail

# banco_emulador.sh — Banco del motor ARM64 dentro del emulador Android.
#
# QUE MIDE:
#   ¿El motor de Factorio ARM64 delivers una imagen NO VACIA por el protocolo
#   XFV1 cuando corre en un Android de verdad con lavapipe (Vulkan por
#   software) en vez de con Turnip (el driver abierto de Adreno)?
#
# POR QUE IMPORTA:
#   En el visor Pico 4 Ultra el motor entra en juego, el inyector se instala
#   y se engancha, pero el framebuffer del mundo sale NEGRO: el inyector
#   descarta casi todos los frames por "vacias" (fraccion_con_contenido < 1 %).
#   En el contenedor (lavapipe) el mismo motor entrega terreno, vias y
#   jugador.  La unica diferencia entre los dos entornos es el driver de
#   Vulkan: Turnip en la Pico, lavapipe en el contenedor.
#
#   Este banco reproduce el caso "Android + lavapipe".  Si aqui llega una
#   imagen con contenido, el camino del inyector es correcto y el problema
#   esta en Turnip.  Si aqui tambien sale negro, el problema es del inyector
#   (o de como el motor compone el framebuffer) y no del driver.
#
# COMO FUNCIONA:
#   El inyector CONECTA HACIA el visor (XFACTORVR_STREAM=host:puerto).  El
#   emulador alcanza el bucle local del Mac en 10.0.2.2, asi que el probe
#   escucha en 127.0.0.1:27020 del Mac y el motor del emulador apunta a
#   10.0.2.2:27020.
#
# Uso:
#   bash scripts/banco_emulador.sh desplegar   # pushes + extraccion
#   bash scripts/banco_emulador.sh arrancar    # servidor + cliente grafico
#   bash scripts/banco_emulador.sh probar     # probe XFV1 + veredicto
#   bash scripts/banco_emulador.sh banco      # todo seguido (desplegar+arrancar+probar)
#   bash scripts/banco_emulador.sh estado

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK="${ANDROID_SDK_ROOT:-/Volumes/X10Pro/Android/sdk}"
SERIAL="${SERIAL:-emulator-5554}"
ADB="${ADB:-$SDK/platform-tools/adb}"

REMOTE_TMP="/data/local/tmp"
REMOTE_MESA="$REMOTE_TMP/mesa"
REMOTE_GLIBC="$REMOTE_TMP/glibc"
REMOTE_CLIENT="$REMOTE_TMP/factorio"
REMOTE_SERVER="$REMOTE_TMP/factorio_arm64"
REMOTE_CLIENT_DIR="$REMOTE_TMP/factorio_cliente"

PROBE_PUERTO="${PROBE_PUERTO:-27020}"
# El emulador ve al Mac por 10.0.2.2 (alias del bucle local del host).
HOST_DESDE_EMULADOR="${HOST_DESDE_EMULADOR:-10.0.2.2}"
SEGUNDOS_PROBE="${SEGUNDOS_PROBE:-45}"

# El ICD de Vulkan del motor.  En el emulador NO se usa Turnip: no hay
# /dev/kgsl-3d0.  Se usa lavapipe (software), que es el caso que queremos
# aislar.  Para el barrido completo (comparar drivers) se puede poner
# XFACTORVR_USE_TURNIP=1 / VK_ICD_FILENAMES a mano.
export XFACTORVR_USE_TURNIP="${XFACTORVR_USE_TURNIP:-0}"

say() { echo "[banco-emu] $*"; }
die() { echo "[banco-emu] ERROR: $*" >&2; exit 1; }

adbx() { "$ADB" -s "$SERIAL" "$@"; }

require_emulador() {
    "$ADB" devices | grep -q "^$SERIAL[[:space:]]*device" || \
        die "el emulador $SERIAL no esta en marcha (bash scripts/emulador.sh iniciar)"
}

# ---------------------------------------------------------------------------
# desplegar
# ---------------------------------------------------------------------------
desplegar() {
    require_emulador
    say "subiendo los payloads al emulador (esto tarda: el cliente grafico pesa 1,6 GB)..."

    say "[1/6] runtime de glibc"
    adbx shell "mkdir -p $REMOTE_GLIBC"
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/build/glibc.tar.gz" "$REMOTE_GLIBC/glibc.tar.gz" >/dev/null
    adbx shell "cd $REMOTE_GLIBC && tar -xzf glibc.tar.gz && chmod 755 lib/ld-linux-aarch64.so.1 && ls -la lib/ld-linux-aarch64.so.1"

    say "[2/6] payload de Mesa (zink + lavapipe)"
    adbx shell "mkdir -p $REMOTE_MESA"
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/build/mesa.tar.gz" "$REMOTE_MESA/mesa.tar.gz" >/dev/null
    adbx shell "cd $REMOTE_TMP && tar -xzf mesa/mesa.tar.gz && chmod 755 mesa/lib/ld-linux-aarch64.so.1 && ls mesa/usr/lib/aarch64-linux-gnu/dri/ | head"

    say "[3/6] servidor headless"
    adbx shell "mkdir -p $REMOTE_SERVER"
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/build/factorio_arm64.tar" "$REMOTE_SERVER/payload.tar" >/dev/null
    adbx shell "cd $REMOTE_SERVER && tar -xf payload.tar && rm -f payload.tar && chmod 755 bin/arm64/factorio && mkdir -p mods"

    say "[4/6] cliente grafico"
    adbx shell "mkdir -p $REMOTE_CLIENT"
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/build/client_grafico.tar" "$REMOTE_CLIENT/client.tar" >/dev/null
    adbx shell "cd $REMOTE_TMP && rm -rf factorio && tar -xf factorio/client.tar && rm -f factorio/client.tar && chmod 755 factorio/bin/arm64/factorio && mkdir -p factorio/mods factorio/config factorio/saves"

    say "[5/6] inyector + resolutor + lista de mods"
    "$ADB" -s "$SERIAL" push \
        "$PROJECT_ROOT/injector-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so" \
        "$REMOTE_TMP/libxfactorvr_injector.so" >/dev/null
    "$ADB" -s "$SERIAL" push \
        "$PROJECT_ROOT/resolver-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_resolver.so" \
        "$REMOTE_TMP/libxfactorvr_resolver.so" >/dev/null
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/scripts/mod-list.json" "$REMOTE_TMP/mod-list.json" >/dev/null

    say "[6/6] mod puente + scripts"
    # El nombre de la carpeta del mod lleva la version (el motor la usa tal
    # cual), asi que se lee del info.json en vez de fijarla a mano.
    MOD_VER=$(python3 -c "import json;print(json.load(open('$PROJECT_ROOT/mods/xfactorvr_bridge/info.json'))['version'])" 2>/dev/null || echo "0.2.0")
    say "      mod puente: xfactorvr_bridge_$MOD_VER"
    adbx shell "mkdir -p $REMOTE_SERVER/mods $REMOTE_CLIENT_DIR/mods"
    adbx push "$PROJECT_ROOT/mods/xfactorvr_bridge/control.lua" "$REMOTE_SERVER/mods/xfactorvr_bridge_$MOD_VER/control.lua" >/dev/null
    adbx push "$PROJECT_ROOT/mods/xfactorvr_bridge/info.json" "$REMOTE_SERVER/mods/xfactorvr_bridge_$MOD_VER/info.json" >/dev/null
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/scripts/start_factorio_client.sh" "$REMOTE_TMP/start_factorio_client.sh" >/dev/null
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/scripts/start_factorio_standalone.sh" "$REMOTE_TMP/start_factorio_standalone.sh" >/dev/null
    adbx shell "chmod 755 $REMOTE_TMP/start_factorio_*.sh"

    say "desplegado."
}

# ---------------------------------------------------------------------------
# arrancar
# ---------------------------------------------------------------------------
arrancar() {
    require_emulador
    say "arrancando el SERVIDOR headless en el emulador..."
    adbx shell "cd $REMOTE_TMP && nohup env XFACTORVR_RESOLVER=$REMOTE_TMP/libxfactorvr_resolver.so \
        sh $REMOTE_TMP/start_factorio_standalone.sh > $REMOTE_TMP/srv.log 2>&1 &"
    sleep 3

    say "arrancando el CLIENTE grafico (lavapipe, sin Turnip)..."
    # El cliente se conecta al servidor local del propio emulador y manda los
    # frames al probe del Mac por 10.0.2.2.
    #
    # Los tres diagnosticos de golpe, para que una sola ejecucion diga TODO:
    #   XFACTORVR_DIAG_PANTALLA=1   compara el framebuffer del MUNDO con el de
    #                                la PANTALLA (FBO 0) y dice cuantos pixeles
    #                                tiene color cada uno.
    #   XFACTORVR_CAPTURA_PANTALLA  guarda el FBO 0 como PNG (la imagen final
    #                                de la ventana, GUI incluida si esta ahi).
    #   XFACTORVR_CAPTURA_GUI=1     engancha la captura del motor (ADR-022 §9).
    adbx shell "cd $REMOTE_TMP && nohup env \
        XFACTORVR_USE_TURNIP=$XFACTORVR_USE_TURNIP \
        SERVER_ADDR=127.0.0.1 \
        GAME_DIR=$REMOTE_CLIENT \
        CLIENT_DIR=$REMOTE_CLIENT_DIR \
        XFACTORVR_STREAM_ADDR=$HOST_DESDE_EMULADOR:$PROBE_PUERTO \
        XFACTORVR_DIAG_PANTALLA=1 \
        XFACTORVR_CAPTURA_PANTALLA=$REMOTE_TMP/pantalla.png \
        XFACTORVR_CAPTURA_GUI=1 \
        XFACTORVR_CAPTURA_GUI_RUTA=$REMOTE_TMP/gui.png \
        WIDTH=640 HEIGHT=360 \
        sh $REMOTE_TMP/start_factorio_client.sh > $REMOTE_TMP/cli_juego.log 2>&1 &"
    sleep 2
    say "cliente lanzado. Logs: $REMOTE_TMP/cli_juego.log y $REMOTE_TMP/srv.log"
}

# ---------------------------------------------------------------------------
# probar
# ---------------------------------------------------------------------------
probar() {
    require_emulador
    say "esperando a que el motor entre en juego..."
    adbx shell "grep -aE 'to\(InGame\)|PlayerJoinGame|Factorio initialised|error|Error' $REMOTE_TMP/cli_juego.log | tail -20" || true

    say "lanzando el probe XFV1 en el Mac ($PROBE_PUERTO) durante ${SEGUNDOS_PROBE}s..."
    rm -f /tmp/probe_emu.log /tmp/frame_emu.png
    nohup python3 "$PROJECT_ROOT/scripts/probe_xfv1.py" \
        --puerto "$PROBE_PUERTO" --segundos "$SEGUNDOS_PROBE" \
        --guardar /tmp/frame_emu.png > /tmp/probe_emu.log 2>&1 &
    PROBE_PID=$!

    # El motor tarda en arrancar (lavapipe es software: va lento).
    sleep 20
    say "--- estado del motor ---"
    adbx shell "tail -30 $REMOTE_TMP/cli_juego.log"
    wait $PROBE_PID
    say "--- probe ---"
    cat /tmp/probe_emu.log
    if [ -f /tmp/frame_emu.png ]; then
        say "frame guardado en /tmp/frame_emu.png"
        ls -la /tmp/frame_emu.png
    fi
}

estado() {
    require_emulador
    adbx shell "ps -A | grep -E 'factorio|ld-linux' | head -10"
    echo "--- cliente ---"
    adbx shell "tail -25 $REMOTE_TMP/cli_juego.log" 2>/dev/null
    echo "--- servidor ---"
    adbx shell "tail -15 $REMOTE_TMP/srv.log" 2>/dev/null
}

case "${1:-banco}" in
    desplegar) desplegar ;;
    arrancar)  arrancar ;;
    probar)    probar ;;
    estado)    estado ;;
    banco)
        desplegar
        arrancar
        probar
        ;;
    *) echo "uso: $0 {desplegar|arrancar|probar|estado|banco}" >&2; exit 2 ;;
esac
