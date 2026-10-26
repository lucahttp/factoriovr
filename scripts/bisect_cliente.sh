#!/usr/bin/env bash
set -uo pipefail

# bisect_cliente.sh — Bisect del cuelgue del cliente grafico en el visor.
#
# El cliente MINIMO (sin shim, sin inyector, sin proxy, --disable-audio,
# ventana 640x360) llega a "Joining game".  El que lanza el script de
# arranque se queda en "Factorio initialised".  Cada variante cambia UNA
# cosa y dice si el motor llega a "Joining game".
#
# Uso: bash scripts/bisect_cliente.sh

ADB="${ADB:-$HOME/Library/Android/sdk/platform-tools/adb}"
REMOTE="/data/local/tmp"
MESA="$REMOTE/mesa/usr/lib/aarch64-linux-gnu"
GLIBC="$REMOTE/glibc/lib"

matar_cliente() {
    $ADB shell 'pat="factorio_cliente/config"; for p in /proc/[0-9]*; do [ "$p" = "/proc/$$" ] && continue; if grep -qa "$pat" $p/cmdline 2>/dev/null; then kill ${p#/proc/} 2>/dev/null; fi; done; sleep 2; true' >/dev/null 2>&1
}

servidor_vivo() {
    # OJO: el log guarda el RCON de corridas anteriores, asi que hay que
    # mirar el PROCESO (el servidor se arranca con --start-server y el
    # cliente con --mp-connect).
    if $ADB shell 'pat="--start-server"; for p in /proc/[0-9]*; do [ "$p" = "/proc/$$" ] && continue; if grep -qa "$pat" $p/cmdline 2>/dev/null; then echo si; break; fi; done' 2>/dev/null | grep -q "si"; then
        return 0
    fi
    echo "  (levanto el servidor)"
    $ADB shell "cd $REMOTE && nohup sh $REMOTE/start_factorio_standalone.sh > $REMOTE/srv.log 2>&1 &" >/dev/null 2>&1
    sleep 15
}

# probar <nombre> <extra_env...>
probar() {
    local nombre="$1"; shift
    local extra="$*"
    matar_cliente
    $ADB shell "cd $REMOTE/factorio && nohup env \
        LD_LIBRARY_PATH=$MESA:$GLIBC \
        LIBGL_DRIVERS_PATH=$MESA/dri \
        __EGL_VENDOR_LIBRARY_DIRS=$REMOTE/mesa/glvnd_egl_vendor \
        MESA_LOADER_DRIVER_OVERRIDE=zink ZINK_DESCRIPTORS=auto \
        VK_ICD_FILENAMES=$REMOTE/mesa/turnip_icd.json \
        SDL_VIDEODRIVER=offscreen SDL_AUDIODRIVER=dummy \
        XFACTORVR_USE_TURNIP=1 $extra \
        $GLIBC/ld-linux-aarch64.so.1 --library-path $MESA:$GLIBC \
        bin/arm64/factorio --config $REMOTE/factorio_cliente/config/config.ini \
        --mp-connect 127.0.0.1:34197 --window-size ${VENTANA:-640x360} ${AUDIO:---disable-audio} \
        > $REMOTE/bisect.log 2>&1 &" >/dev/null 2>&1
    sleep ${ESPERA:-35}
    local res
    res="$($ADB shell "grep -ac 'Joining game' $REMOTE/bisect.log" 2>/dev/null | tr -d '\r')"
    if [ "${res:-0}" -gt 0 ] 2>/dev/null; then
        echo "  [OK]   $nombre -> llega a Joining game"
        return 0
    fi
    local donde
    donde="$($ADB shell "grep -aE 'initialised|Joining|failed|Error' $REMOTE/bisect.log 2>/dev/null | tail -1 | cut -c1-90" | tr -d '\r')"
    echo "  [CUELGA] $nombre -> ${donde:-nada}"
    return 1
}

servidor_vivo

echo "== Bisect del cliente =="
probar "1. minimo (referencia)"
probar "2. + shim (resolver)"            "LD_PRELOAD=$REMOTE/libxfactorvr_resolver.so"
probar "3. + inyector"                   "LD_PRELOAD=$REMOTE/libxfactorvr_injector.so"
probar "4. + proxy"                      "http_proxy=http://127.0.0.1:8899 https_proxy=http://127.0.0.1:8899"
probar "5. + shim + inyector + proxy"    "LD_PRELOAD=$REMOTE/libxfactorvr_resolver.so:$REMOTE/libxfactorvr_injector.so http_proxy=http://127.0.0.1:8899 https_proxy=http://127.0.0.1:8899"
AUDIO=" " probar "6. sin --disable-audio"
AUDIO=" " VENTANA=1280x720 probar "7. ventana 1280x720"
