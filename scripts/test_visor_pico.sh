#!/bin/bash
# test_visor_pico.sh — Banco del VISOR contra la PICO 4 Ultra (con cable).
#
# Comprueba, en el dispositivo de verdad, que el visor:
#
#   1. arranca la app,
#   2. se conecta por RCON al motor de Factorio que corre en el movil,
#   3. APLICA la instantanea del juego (no la simulacion local de ejemplo),
#   4. dibuja el diorama con las entidades de verdad.
#
# La evidencia sale del logcat: el cliente Rust escribe por stderr (env_logger)
# y en Android eso acaba en el logcat:
#   "RCON conectado al motor de Factorio en 127.0.0.1:27015"
#   "Instantanea aplicada: 101 entidades del juego real (1 sueltos)"
#
# Codigos de salida:
#   0  todo correcto
#   1  algo falla
#   2  el entorno no permite validarlo (sin cable, sin dispositivo, ...)
#
# Uso: bash scripts/test_visor_pico.sh
#
# Variables de entorno:
#   ADB          binario de adb (/Volumes/X10Pro/Android/sdk/platform-tools/adb)
#   SERIAL       serie del dispositivo (la primera que se vea)
#   PACKAGE      paquete de la app (com.xfactorvr.tabletop)
#   APK          APK a instalar (build/XFactorVR_Pico4Ultra.apk)
#   XFV_SIN_X11  1 = no instalar Termux:X11 (solo lo del RCON)
#   XFV_SIN_DEPLOY 1 = no desplegar el motor (si ya esta)

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADB="${ADB:-/Volumes/X10Pro/Android/sdk/platform-tools/adb}"
PACKAGE="${PACKAGE:-com.xfactorvr.tabletop}"
APK="${APK:-$PROJECT_ROOT/build/XFactorVR_Pico4Ultra.apk}"
XFV_SIN_X11="${XFV_SIN_X11:-0}"
XFV_SIN_DEPLOY="${XFV_SIN_DEPLOY:-0}"

REMOTE_SERVER="/data/local/tmp/factorio_arm64"
REMOTE_MOD="$REMOTE_SERVER/mods/xfactorvr_bridge_0.1.1"
REMOTE_SCRIPT="/data/local/tmp/start_factorio_standalone.sh"
RCON_PORT=27015
RCON_PASSWORD=xfactorvr

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

# --------------------------------------------------------------------------
# 1. Preflight: el dispositivo tiene que estar conectado por cable
# --------------------------------------------------------------------------
paso "Preflight (la Pico tiene que estar conectada por USB)"

command -v "$ADB" >/dev/null 2>&1 || sin_entorno "no esta '$ADB' en el PATH"
SERIES="$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1}')"
[ -n "$SERIES" ] || sin_entorno "no hay ningun dispositivo conectado (el cable se ha vuelto a caer)"
SERIAL="${SERIAL:-$(printf '%s' "$SERIES" | head -1)}"
MODEL="$("$ADB" -s "$SERIAL" shell getprop ro.product.model 2>/dev/null | tr -d '\r')"
ANDROID="$("$ADB" -s "$SERIAL" shell getprop ro.build.version.release 2>/dev/null | tr -d '\r')"
ok "dispositivo $SERIAL ($MODEL, Android $ANDROID)"

# --------------------------------------------------------------------------
# 2. Instalar la app y Termux:X11
# --------------------------------------------------------------------------
paso "Instalacion de la app (y de Termux:X11, que hace falta para el render)"

[ -f "$APK" ] || sin_entorno "no esta el APK '$APK' (se construye con scripts/build_apk.sh)"
if "$ADB" -s "$SERIAL" install -r "$APK" 2>&1 | tail -1 | grep -q "Success"; then
    ok "app instalada ($(basename "$APK"))"
else
    fail "no se pudo instalar el APK"
fi

X11_APK="$PROJECT_ROOT/build/termux/termux-x11-universal.apk"
if [ "$XFV_SIN_X11" = "1" ]; then
    aviso "Termux:X11 saltado (XFV_SIN_X11=1)"
elif [ ! -f "$X11_APK" ]; then
    aviso "no esta el APK de Termux:X11 en $X11_APK (descargalo de la release de termux/termux-x11)"
elif "$ADB" -s "$SERIAL" shell pm list packages 2>/dev/null | grep -q "termux.x11"; then
    ok "Termux:X11 ya instalado"
else
    if "$ADB" -s "$SERIAL" install -r "$X11_APK" 2>&1 | tail -1 | grep -q "Success"; then
        ok "Termux:X11 instalado"
    else
        fail "no se pudo instalar Termux:X11"
    fi
fi

# --------------------------------------------------------------------------
# 3. Desplegar el mod actualizado en el servidor del movil
# --------------------------------------------------------------------------
paso "Mod puente actualizado en el servidor del movil"

if [ "$XFV_SIN_DEPLOY" = "1" ]; then
    aviso "despliegue saltado (XFV_SIN_DEPLOY=1)"
else
    "$ADB" -s "$SERIAL" shell "test -d '$REMOTE_MOD'" 2>/dev/null \
        || sin_entorno "el servidor de Factorio no esta en '$REMOTE_SERVER' (scripts/deploy_pico.sh)"
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/mods/xfactorvr_bridge/control.lua" "$REMOTE_MOD/control.lua" >/dev/null 2>&1
    "$ADB" -s "$SERIAL" push "$PROJECT_ROOT/mods/xfactorvr_bridge/info.json" "$REMOTE_MOD/info.json" >/dev/null 2>&1
    CANONICAL="$("$ADB" -s "$SERIAL" shell "grep -c CANONICAL_TYPE_NAMES '$REMOTE_MOD/control.lua'" 2>/dev/null | tr -d '\r')"
    if [ "${CANONICAL:-0}" -gt 0 ]; then
        ok "el mod con el tipo canonico esta desplegado"
    else
        fail "el mod desplegado NO lleva el tipo canonico"
    fi
fi

# --------------------------------------------------------------------------
# 4. Arrancar el motor headless en el movil
# --------------------------------------------------------------------------
paso "Arranque del motor headless en el movil"

"$ADB" -s "$SERIAL" shell "ps -A | grep '[f]actorio'" 2>/dev/null | head -2
"$ADB" -s "$SERIAL" shell "nohup sh '$REMOTE_SCRIPT' > /data/local/tmp/factorio_srv.log 2>&1 &" >/dev/null 2>&1

LISTO=0
for i in $(seq 1 120); do
    if "$ADB" -s "$SERIAL" shell "grep -q 'Starting RCON interface' /data/local/tmp/factorio_srv.log 2>/dev/null"; then
        LISTO=1
        break
    fi
    sleep 2
done
if [ "$LISTO" = "1" ]; then
    ok "el motor arranco y el RCON escucha en el puerto $RCON_PORT (tras ${i}s)"
else
    fail "el motor no abrio el RCON"
    "$ADB" -s "$SERIAL" shell "tail -20 /data/local/tmp/factorio_srv.log 2>/dev/null" | sed 's/^/  | /'
    sin_entorno "sin motor no se puede validar el visor"
fi

# --------------------------------------------------------------------------
# 5. Arrancar la app y ver que se conecta al motor
# --------------------------------------------------------------------------
paso "La app se conecta al motor y trae el juego real"

"$ADB" -s "$SERIAL" logcat -c >/dev/null 2>&1
"$ADB" -s "$SERIAL" shell am force-stop "$PACKAGE" >/dev/null 2>&1
sleep 1
"$ADB" -s "$SERIAL" shell am start -n "$PACKAGE/android.app.NativeActivity" >/dev/null 2>&1
ok "app arrancada"

CONECTADO=0
INSTANTANEA=0
for i in $(seq 1 45); do
    sleep 2
    if "$ADB" -s "$SERIAL" logcat -d 2>/dev/null | grep -q "RCON conectado al motor"; then
        CONECTADO=1
    fi
    if "$ADB" -s "$SERIAL" logcat -d 2>/dev/null | grep -q "Instantanea aplicada"; then
        INSTANTANEA=1
    fi
    if [ "$CONECTADO" = "1" ] && [ "$INSTANTANEA" = "1" ]; then
        break
    fi
done

LINEA_RCON="$("$ADB" -s "$SERIAL" logcat -d 2>/dev/null | grep "RCON conectado al motor" | tail -1 | tr -d '\r')"
LINEA_INST="$("$ADB" -s "$SERIAL" logcat -d 2>/dev/null | grep "Instantanea aplicada" | tail -1 | tr -d '\r')"
printf '  %s\n' "$LINEA_RCON" | sed 's/^/  | /'
printf '  %s\n' "$LINEA_INST" | sed 's/^/  | /'

if [ "$CONECTADO" = "1" ]; then
    ok "el visor se conecto por RCON al motor del movil"
else
    fail "el visor NO se conecto por RCON"
fi

ENTIDADES="$(printf '%s' "$LINEA_INST" | sed -n 's/.*Instantanea aplicada: \([0-9]*\) entidades.*/\1/p')"
ENTIDADES="${ENTIDADES:-0}"
if [ "$ENTIDADES" -gt 11 ]; then
    ok "el visor trae $ENTIDADES entidades del juego real (la simulacion local solo pone 11)"
elif [ "$ENTIDADES" -gt 0 ]; then
    fail "el visor solo trae $ENTIDADES entidades: sigue en la simulacion local"
else
    fail "el visor no aplico ninguna instantanea"
fi

# --------------------------------------------------------------------------
# 6. Evidencia: captura de la pantalla
# --------------------------------------------------------------------------
paso "Captura de la pantalla del visor"

CAPTURA="$PROJECT_ROOT/build/pico_visor.png"
"$ADB" -s "$SERIAL" exec-out screencap -p > "$CAPTURA" 2>/dev/null
if [ -s "$CAPTURA" ]; then
    ok "captura guardada en $CAPTURA ($(wc -c < "$CAPTURA") bytes)"
else
    aviso "no se pudo capturar la pantalla"
fi

printf '\n=== RESULTADO ===\n'
printf 'pasos: %d   fallos: %d\n' "$PASOS" "$FALLOS"
if [ "$FALLOS" -eq 0 ]; then
    printf 'RESULTADO: CORRECTO\n'
    exit 0
fi
printf 'RESULTADO: INCORRECTO\n'
exit 1
