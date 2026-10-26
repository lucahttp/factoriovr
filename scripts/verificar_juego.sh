#!/usr/bin/env bash
set -uo pipefail

# verificar_juego.sh — Prueba de aceptacion en el visor: "se puede jugar".
#
# Hace toda la cadena, en el visor, y dice si se puede jugar:
#
#   1. Sube los scripts, la lista canonica de mods y el resolutor.
#   2. Levanta el servidor headless y espera a que hospeda la partida.
#   3. Levanta el cliente grafico y espera a que ENTRE como jugador
#      (PlayerJoinGame / estado InGame en el motor).
#   4. Comprueba que el estado por fichero lleva la posicion REAL del
#      jugador (la camara de la mesa depende de esto).
#   5. Comprueba que el inyector sigue vivo dentro del cliente (el render 3D
#      y la geometria dependen de esto).
#   6. Comprueba que el visor (la app) esta leyendo el estado y la
#      geometria, y saca una captura de lo que se ve.
#
# Codigos de salida:
#   0  se puede jugar
#   1  algo falla (dice que)
#   2  no validable (sin visor, sin lo que hace falta, ...)
#
# Uso: bash scripts/verificar_juego.sh

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADB="${ADB:-$HOME/Library/Android/sdk/platform-tools/adb}"
REMOTE="/data/local/tmp"
SITIO="${SITIO:-/sdcard/XFactorVR}"
ESTADO="$SITIO/srv/script-output/xfactorvr_bridge/estado.json"
CAPTURA="${CAPTURA:-$PROJECT_ROOT/build/visor_juego.png}"

FALLOS=0
fallo() { echo "  FALLO: $*"; FALLOS=$((FALLOS + 1)); }
ok()    { echo "  ok: $*"; }
paso()  { echo ""; echo "=== $* ==="; }

if ! "$ADB" devices | grep -q "device$"; then
    echo "NO VALIDABLE: no hay visor conectado por adb."
    exit 2
fi

paso "1. Subiendo lo nuevo"
"$ADB" push "$PROJECT_ROOT/scripts/start_factorio_client.sh" "$REMOTE/start_factorio_client.sh" >/dev/null
"$ADB" push "$PROJECT_ROOT/scripts/start_factorio_standalone.sh" "$REMOTE/start_factorio_standalone.sh" >/dev/null
"$ADB" push "$PROJECT_ROOT/scripts/mod-list.json" "$REMOTE/mod-list.json" >/dev/null
RESOLVER="$PROJECT_ROOT/resolver-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_resolver.so"
if [ -f "$RESOLVER" ]; then
    "$ADB" push "$RESOLVER" "$REMOTE/libxfactorvr_resolver.so" >/dev/null
    "$ADB" shell "chmod 755 $REMOTE/libxfactorvr_resolver.so" >/dev/null
    ok "resolutor de nombres actualizado"
else
    echo "  AVISO: sin resolutor compilado (se usa el que hay en el visor)"
fi
"$ADB" shell "chmod 755 $REMOTE/start_factorio_client.sh $REMOTE/start_factorio_standalone.sh" >/dev/null
ok "scripts y lista de mods subidos"

paso "2. Reiniciando el motor"
# El proceso del motor en el visor se llama "ld-linux-aarch64.so.1", asi que
# se mata por la linea de ordenes, no por el nombre.
#
# OJO: el patron se arma con un "/" DELANTE de "factorio".  Si se concatena
# sin separador queda "bin/arm64factorio", que no casa con nada y el motor
# sigue vivo (y el script cree que ya habia un servidor).
"$ADB" shell 'pat="bin/arm64"; pat="${pat}/factorio"; for p in /proc/[0-9]*; do [ "$p" = "/proc/$$" ] && continue; if grep -qa "$pat" $p/cmdline 2>/dev/null; then kill ${p#/proc/} 2>/dev/null; fi; done; sleep 3; true' >/dev/null 2>&1
VIVOS="$("$ADB" shell 'pat="bin/arm64"; pat="${pat}/factorio"; for p in /proc/[0-9]*; do [ "$p" = "/proc/$$" ] && continue; if grep -qa "$pat" $p/cmdline 2>/dev/null; then echo "${p#/proc/}"; fi; done' 2>/dev/null | tr -d '\r' | tr '\n' ' ')"
if [ -n "${VIVOS// /}" ]; then
    fallo "quedan motores vivos (PIDs: ${VIVOS})"
else
    ok "motores parados"
fi

paso "3. Levantando el servidor"
"$ADB" shell "cd $REMOTE && XFACTORVR_RESOLVER_LOG=$REMOTE/res_srv.log nohup sh $REMOTE/start_factorio_standalone.sh > $REMOTE/srv.log 2>&1 &" >/dev/null
SERVER_LOG="$REMOTE/factorio_arm64/logs/standalone.log"
for i in $(seq 1 60); do
    if "$ADB" shell "grep -q 'to(InGame)' '$SERVER_LOG'" >/dev/null 2>&1; then break; fi
    sleep 2
done
if "$ADB" shell "grep -q 'to(InGame)' '$SERVER_LOG'" >/dev/null 2>&1; then
    ok "el servidor hospeda la partida"
else
    fallo "el servidor no llega a InGame"
    "$ADB" shell "tail -15 '$SERVER_LOG'" 2>&1 | sed 's/^/    /'
    exit 1
fi

paso "4. Levantando el cliente grafico"
"$ADB" shell "cd $REMOTE && nohup env XFACTORVR_USE_TURNIP=1 SERVER_ADDR=127.0.0.1 GAME_DIR=$REMOTE/factorio sh $REMOTE/start_factorio_client.sh > $REMOTE/cli.log 2>&1 &" >/dev/null
CONECTADO=0
for i in $(seq 1 90); do
    if "$ADB" shell "grep -q 'PlayerJoinGame' '$SERVER_LOG'" >/dev/null 2>&1; then CONECTADO=1; break; fi
    sleep 2
done
if [ "$CONECTADO" = "1" ]; then
    ok "el cliente entra como jugador"
    "$ADB" shell "grep -a 'PlayerJoinGame' '$SERVER_LOG' | tail -1" 2>&1 | sed 's/^/    /'
else
    fallo "el cliente NO entra como jugador"
    echo "--- log del cliente ---"
    "$ADB" shell "tail -20 $REMOTE/cli.log" 2>&1 | sed 's/^/    /'
    echo "--- log propio del cliente ---"
    "$ADB" shell "tail -12 $REMOTE/factorio_cliente/factorio-current.log" 2>&1 | sed 's/^/    /'
    exit 1
fi

paso "5. La camara (posicion del jugador en el estado por fichero)"
# El mod empuja la posicion del jugador a estado.json; el visor la usa para
# poner la mesa donde esta la fabrica.  Sin esto el visor se queda en (0,0).
ESPERA=0
while [ $ESPERA -lt 20 ]; do
    if "$ADB" shell "test -f '$ESTADO'" >/dev/null 2>&1; then break; fi
    sleep 2
    ESPERA=$((ESPERA + 2))
done
if "$ADB" shell "test -f '$ESTADO'" >/dev/null 2>&1; then
    JUGADOR="$("$ADB" shell "cat '$ESTADO'" 2>/dev/null | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception:
    print(''); raise SystemExit
j = d.get('jugador') or {}
print('%s,%s' % (j.get('x'), j.get('y')))
" 2>/dev/null)"
    echo "    jugador en estado.json: ${JUGADOR:-?}"
    if [ -n "$JUGADOR" ] && [ "${JUGADOR%,*}" != "None" ] && [ "${JUGADOR%,*}" != "0" ]; then
        ok "el estado lleva la posicion del jugador (no 0,0)"
    else
        fallo "el estado NO lleva la posicion del jugador"
    fi
else
    fallo "no hay estado en $ESTADO"
fi

paso "6. El inyector y el visor"
HOOKS="$("$ADB" shell "grep -ac 'hook vivo' $REMOTE/cli.log" 2>/dev/null | tr -d '\r' | tr -d '[:space:]')"
if [ "${HOOKS:-0}" -gt 0 ] 2>/dev/null; then
    ok "el inyector sigue vivo dentro del cliente ($HOOKS avisos)"
else
    fallo "el inyector no engancho el render del cliente"
fi

"$ADB" shell "am force-stop com.xfactorvr.tabletop" >/dev/null 2>&1
"$ADB" shell "am start -n com.xfactorvr.tabletop/android.app.NativeActivity" >/dev/null 2>&1
sleep 8
METRICAS="$("$ADB" shell "logcat -d | grep -aE 'PxrMetric|XFactorVR_OpenXR' | tail -8" 2>/dev/null)"
if [ -n "$METRICAS" ]; then
    ok "el visor arranco (metricas en el logcat)"
    printf '%s\n' "$METRICAS" | sed 's/^/    /'
else
    fallo "el visor no deja metricas en el logcat"
fi

mkdir -p "$(dirname "$CAPTURA")"
"$ADB" exec-out screencap -p > "$CAPTURA" 2>/dev/null
if [ -s "$CAPTURA" ]; then
    ok "captura del visor: $CAPTURA"
else
    fallo "no se pudo capturar la pantalla del visor"
fi

echo ""
if [ $FALLOS -eq 0 ]; then
    echo "RESULTADO: SE PUEDE JUGAR (0 fallos)"
    exit 0
fi
echo "RESULTADO: AUN NO (${FALLOS} fallos)"
exit 1
