#!/usr/bin/env bash
set -uo pipefail

# vigilar_visor.sh — Vigilante: espera al visor, despliega y verifica.
#
# El cable USB de la Pico 4 Ultra se suelta cada dos por tres, asi que esto
# se queda esperando: en cuanto el visor aparece por adb, sube lo nuevo
# (inyector con la deteccion del motor por /proc/self/maps, resolutor con la
# lista de DNS dinamica, scripts y lista de mods), levanta el servidor y el
# cliente, y comprueba si el cliente entra como jugador.
#
# El resultado se escribe en /tmp/vigila_visor.log y en /tmp/vigila_visor.res
# (OK / FALLO + detalle).
#
# Uso: bash scripts/vigilar_visor.sh [intentos]

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADB="${ADB:-$HOME/Library/Android/sdk/platform-tools/adb}"
REMOTE="/data/local/tmp"
INTENTOS="${1:-200}"
LOG="/tmp/vigila_visor.log"
RES="/tmp/vigila_visor.res"

say() { printf '%s %s\n' "$(date '+%H:%M:%S')" "$*" >> "$LOG"; }

conectado() { $ADB devices 2>/dev/null | grep -q "device$"; }

# Reintenta hasta que adb vea el visor (el USB枚举a veces se cuelga).
espera_dispositivo() {
    local i=0
    while [ $i -lt 60 ]; do
        if conectado; then return 0; fi
        $ADB kill-server >/dev/null 2>&1
        $ADB start-server >/dev/null 2>&1
        sleep 5
        i=$((i + 1))
    done
    return 1
}

say "=== vigilante arrancado ==="

for intento in $(seq 1 "$INTENTOS"); do
    if ! espera_dispositivo; then
        say "intento $intento: el visor sigue sin verse por adb"
        sleep 10
        continue
    fi
    say "intento $intento: VISOR CONECTADO"

    # 1. Desplegar lo nuevo.
    $ADB push "$PROJECT_ROOT/injector-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so" \
        "$REMOTE/libxfactorvr_injector.so" >> "$LOG" 2>&1
    $ADB push "$PROJECT_ROOT/resolver-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_resolver.so" \
        "$REMOTE/libxfactorvr_resolver.so" >> "$LOG" 2>&1
    $ADB push "$PROJECT_ROOT/scripts/start_factorio_client.sh" "$REMOTE/start_factorio_client.sh" >> "$LOG" 2>&1
    $ADB push "$PROJECT_ROOT/scripts/start_factorio_standalone.sh" "$REMOTE/start_factorio_standalone.sh" >> "$LOG" 2>&1
    $ADB push "$PROJECT_ROOT/scripts/mod-list.json" "$REMOTE/mod-list.json" >> "$LOG" 2>&1
    $ADB shell "chmod 755 $REMOTE/libxfactorvr_injector.so $REMOTE/libxfactorvr_resolver.so $REMOTE/start_factorio_client.sh $REMOTE/start_factorio_standalone.sh" >> "$LOG" 2>&1
    say "desplegado"

    # 1 bis. APK del visor (el HUD propio ya no tapa el juego).
    if [ -f "$PROJECT_ROOT/build/XFactorVR_Pico4Ultra.apk" ]; then
        if $ADB install -r "$PROJECT_ROOT/build/XFactorVR_Pico4Ultra.apk" >> "$LOG" 2>&1; then
            say "APK del visor instalado"
            $ADB shell "am force-stop com.xfactorvr.tabletop" >> "$LOG" 2>&1
            $ADB shell "am start -n com.xfactorvr.tabletop/android.app.NativeActivity" >> "$LOG" 2>&1
            sleep 6
        else
            say "AVISO: no se pudo instalar el APK (sigue el anterior)"
        fi
    fi

    # 1 ter. Apartar las expansiones del read-data del SERVIDOR (si no, el
    # servidor y el cliente tienen listas distintas y el servidor rechaza la
    # conexion con ModsMismatch).
    $ADB shell 'for e in space-age quality recycler elevated-rails; do if [ -d /data/local/tmp/factorio_arm64/data/$e ]; then mkdir -p /data/local/tmp/factorio_arm64/data/_sin_licencia; mv /data/local/tmp/factorio_arm64/data/$e /data/local/tmp/factorio_arm64/data/_sin_licencia/ && echo "apartada $e"; fi; done' >> "$LOG" 2>&1

    # 2. Reiniciar los motores (el cliente y el servidor).
    $ADB shell 'pat="bin/arm64"; pat="${pat}/factorio"; for p in /proc/[0-9]*; do [ "$p" = "/proc/$$" ] && continue; if grep -qa "$pat" $p/cmdline 2>/dev/null; then kill ${p#/proc/} 2>/dev/null; fi; done; sleep 3; true' >> "$LOG" 2>&1
    $ADB shell "cd $REMOTE && XFACTORVR_RESOLVER_LOG=$REMOTE/res_srv.log nohup sh $REMOTE/start_factorio_standalone.sh > $REMOTE/srv.log 2>&1 &" >> "$LOG" 2>&1
    sleep 18
    SERVIDOR="$($ADB shell "grep -ac 'to(InGame)' $REMOTE/factorio_arm64/logs/standalone.log" 2>/dev/null | tr -d '\r')"
    say "servidor InGame: $SERVIDOR"

    $ADB shell "cd $REMOTE && nohup env XFACTORVR_USE_TURNIP=1 SERVER_ADDR=127.0.0.1 GAME_DIR=$REMOTE/factorio XFACTORVR_RESOLVER_LOG=$REMOTE/res_cli.log sh $REMOTE/start_factorio_client.sh > $REMOTE/cli.log 2>&1 &" >> "$LOG" 2>&1

    # 3. Esperar a que el cliente entre.
    CONECTADO=0
    for i in $(seq 1 60); do
        if $ADB shell "grep -q 'PlayerJoinGame' $REMOTE/factorio_arm64/logs/standalone.log" >/dev/null 2>&1; then
            CONECTADO=1
            break
        fi
        sleep 3
    done

    if [ "$CONECTADO" = "1" ]; then
        say "*** EL CLIENTE ENTRA COMO JUGADOR ***"
        echo "OK: el cliente entra como jugador" > "$RES"
        # Estado del mundo (la camara) y el inyector.
        sleep 5
        $ADB shell "cat $REMOTE/factorio_arm64/logs/standalone.log | grep -a 'PlayerJoinGame' | tail -1" >> "$LOG" 2>&1
        $ADB shell "grep -aE 'hook vivo|xfv-injector' $REMOTE/cli.log | tail -8" >> "$LOG" 2>&1
        $ADB shell "cat /sdcard/XFactorVR/srv/script-output/xfactorvr_bridge/estado.json 2>/dev/null | head -c 400" >> "$LOG" 2>&1
        exit 0
    fi

    say "el cliente NO entra (todavia)"
    $ADB shell "tail -6 $REMOTE/cli.log" >> "$LOG" 2>&1
    $ADB shell "grep -aE 'xfv-injector|hook vivo|objeto cargado' $REMOTE/cli.log | tail -12" >> "$LOG" 2>&1
    echo "FALLO: el cliente no entra como jugador (intento $intento)" > "$RES"
    sleep 15
done

say "el vigilante se cansa: el visor no ha vuelto"
echo "FALLO: el visor no ha vuelto" > "$RES"
exit 1
