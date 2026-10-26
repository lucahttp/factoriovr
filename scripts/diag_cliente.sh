#!/usr/bin/env bash
set -uo pipefail

# diag_cliente.sh — Diagnostico del cliente grafico en el visor.
#
# El cliente se queda parado en "Factorio initialised" sin llegar a
# "Joining game".  Este script lanza el cliente de varias formas para
# aislar la causa y deja los logs en /data/local/tmp/diag/.
#
# Uso: bash scripts/diag_cliente.sh [variante]
#   variantes: minimo (sin shim ni inyector), completo (todo), auto

ADB="${ADB:-$HOME/Library/Android/sdk/platform-tools/adb}"
REMOTE="/data/local/tmp"
DIAG="$REMOTE/diag"
VARIANTE="${1:-auto}"

$ADB shell "mkdir -p $DIAG" >/dev/null 2>&1

matar() {
    # SOLO el cliente: el servidor tiene que seguir vivo, o el cliente no
    # tiene a quien conectarse y el diagnostico no dice nada.
    $ADB shell 'pat="factorio_cliente/config"; for p in /proc/[0-9]*; do [ "$p" = "/proc/$$" ] && continue; if grep -qa "$pat" $p/cmdline 2>/dev/null; then kill ${p#/proc/} 2>/dev/null; fi; done; sleep 3; true' >/dev/null 2>&1
}

# El servidor tiene que estar en marcha (y con el RCON abierto).
levantar_servidor() {
    if ! $ADB shell 'pat="--start-server"; for p in /proc/[0-9]*; do [ "$p" = "/proc/$$" ] && continue; if grep -qa "$pat" $p/cmdline 2>/dev/null; then echo si; break; fi; done' 2>/dev/null | grep -q "si"; then
        echo "  (el servidor no esta en marcha: lo levanto)"
        $ADB shell "cd $REMOTE && XFACTORVR_RESOLVER_LOG=$DIAG/res_srv.log nohup sh $REMOTE/start_factorio_standalone.sh > $REMOTE/srv.log 2>&1 &" >/dev/null 2>&1
        sleep 15
    fi
    $ADB shell "grep -a 'to(InGame)' /data/local/tmp/factorio_arm64/logs/standalone.log | tail -1" 2>&1 | sed 's/^/  servidor: /'
}

# --- El cliente minimo: solo el motor, sin shim ni inyector ---------------
minimo() {
    echo "== Cliente MINIMO (sin resolver, sin inyector) =="
    levantar_servidor
    matar
    $ADB shell "cd $REMOTE/factorio && nohup env \
        LD_LIBRARY_PATH=$REMOTE/mesa/usr/lib/aarch64-linux-gnu:$REMOTE/glibc/lib \
        LIBGL_DRIVERS_PATH=$REMOTE/mesa/usr/lib/aarch64-linux-gnu/dri \
        __EGL_VENDOR_LIBRARY_DIRS=$REMOTE/mesa/glvnd_egl_vendor \
        MESA_LOADER_DRIVER_OVERRIDE=zink ZINK_DESCRIPTORS=auto \
        VK_ICD_FILENAMES=$REMOTE/mesa/turnip_icd.json \
        SDL_VIDEODRIVER=offscreen SDL_AUDIODRIVER=dummy \
        XFACTORVR_USE_TURNIP=1 \
        $REMOTE/glibc/lib/ld-linux-aarch64.so.1 --library-path $REMOTE/mesa/usr/lib/aarch64-linux-gnu:$REMOTE/glibc/lib \
        bin/arm64/factorio --config $REMOTE/factorio_cliente/config/config.ini \
        --mp-connect 127.0.0.1:34197 --window-size 640x360 --disable-audio \
        > $DIAG/minimo.log 2>&1 &" >/dev/null 2>&1
    sleep 40
    $ADB shell "grep -aE 'initialised|Joining|PlayerJoin|Error|error' $DIAG/minimo.log | tail -6" 2>&1 | sed 's/^/  /'
    $ADB shell "grep -ac 'PlayerJoinGame' $REMOTE/factorio_arm64/logs/standalone.log" 2>&1 | sed 's/^/  PlayerJoinGame en el servidor: /'
}

# --- El cliente completo: con shim, inyector y el log del resolver --------
completo() {
    echo "== Cliente COMPLETO (shim + inyector + log del resolver) =="
    levantar_servidor
    matar
    $ADB shell "cd $REMOTE && nohup env \
        XFACTORVR_USE_TURNIP=1 SERVER_ADDR=127.0.0.1 GAME_DIR=$REMOTE/factorio \
        XFACTORVR_RESOLVER_LOG=$DIAG/res_cli.log \
        sh $REMOTE/start_factorio_client.sh > $DIAG/completo.log 2>&1 &" >/dev/null 2>&1
    sleep 40
    echo "  --- log del motor ---"
    $ADB shell "grep -aE 'initialised|Joining|PlayerJoin|Error' $DIAG/completo.log | tail -6" 2>&1 | sed 's/^/  /'
    echo "  --- inyector (donde esta el motor) ---"
    $ADB shell "grep -aE 'xfv-injector|xfactorvr' $DIAG/completo.log | tail -12" 2>&1 | sed 's/^/  /'
    echo "  --- log del shim (conexiones) ---"
    $ADB shell "tail -10 $DIAG/res_cli.log 2>/dev/null" 2>&1 | sed 's/^/  /'
    echo "  --- pila de estados del motor (diagnostico del inyector) ---"
    $ADB shell "grep -a 'diag:' $DIAG/completo.log | tail -6" 2>&1 | sed 's/^/  /'
}

case "$VARIANTE" in
    minimo) minimo ;;
    completo) completo ;;
    auto) minimo; echo; completo ;;
    *) echo "variante desconocida: $VARIANTE"; exit 2 ;;
esac
