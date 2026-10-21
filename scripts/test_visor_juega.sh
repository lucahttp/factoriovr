#!/bin/bash
# test_visor_juega.sh — Banco: el VISOR sabe jugar.
#
# Comprueba, con las MISMAS funciones que usa el APK (el crate xfactorvr: el
# puente RCON y las ordenes del mod), que:
#
#   1. el puente RCON conecta con el motor de Factorio,
#   2. llega la instantanea del mundo CON el terreno,
#   3. el jugador se mueve y el motor lo refleja,
#   4. se puede minar y fabricar.
#
# Es la parte del visor que decide si se puede jugar la campana.  Se puede
# comprobar sin el visor: el binario corre en el host y habla con el motor
# real a traves del RCON (el de la Pico con adb forward, o el del contenedor).
#
# Uso: bash scripts/test_visor_juega.sh [pico|contenedor]
# Salida: 0 si todo va bien, 1 si algo falla, 2 si no se puede validar.
set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PODMAN="${PODMAN:-/opt/podman/bin/podman}"
ADB="${ADB:-/Volumes/X10Pro/Android/sdk/platform-tools/adb}"
CONTAINER="xfactorvr-client"
RCON_PORT=27015
RCON_PASSWORD=xfactorvr
MODO="${1:-pico}"

say()  { printf '%s\n' "$*"; }
ok()   { printf '  OK   %s\n' "$*"; }
fail() { printf 'FALLO: %s\n' "$*" >&2; exit 1; }

say "=== 1. Compilando la prueba del visor ==="
if ! cargo build --release --bin probar_visor 2>/dev/null; then
    fail "no se pudo compilar el binario probar_visor"
fi
BIN="$PROJECT_ROOT/target/release/probar_visor"
[ -x "$BIN" ] || fail "no existe $BIN"
ok "binario listo"

PUERTO=0

if [ "$MODO" = "pico" ]; then
    say ""
    say "=== 2. Publicando el RCON del visor ==="
    if ! "$ADB" devices | grep -q "device$"; then
        say "AVISO: la Pico no esta conectada: no se puede validar"
        exit 2
    fi
    # El motor del visor lo arranca scripts/deploy_pico.sh; si no esta, se
    # avisa (el banco no lo levanta: el motor necesita el shim y el proxy).
    if ! "$ADB" shell "netstat -tln 2>/dev/null | grep -q ':$RCON_PORT'"; then
        say "AVISO: no hay motor en la Pico. Arrancalo con:"
        say "         adb shell 'SCENARIO=base/xfv-nivel sh /data/local/tmp/start_factorio_standalone.sh'"
        exit 2
    fi
    "$ADB" forward "tcp:$RCON_PORT" "tcp:$RCON_PORT" >/dev/null 2>&1 || true
    PUERTO="$RCON_PORT"
    ok "RCON de la Pico publicado en 127.0.0.1:$PUERTO"
else
    say ""
    say "=== 2. Publicando el RCON del contenedor ==="
    if ! "$PODMAN" container exists "$CONTAINER" 2>/dev/null; then
        say "AVISO: no existe el contenedor $CONTAINER: no se puede validar"
        exit 2
    fi
    if ! "$PODMAN" exec "$CONTAINER" bash -c "exec 3<>/dev/tcp/127.0.0.1/$RCON_PORT" 2>/dev/null; then
        say "AVISO: no hay motor en el contenedor: no se puede validar"
        exit 2
    fi
    # El host no alcanza la red interna del contenedor (slirp no hace NAT
    # inverso) y la maquina de podman se maneja por socket local, sin puerto
    # TCP al que abrir un tunel.  La via que queda es la de verdad: el
    # dispositivo.
    say "AVISO: el contenedor no publica el RCON al host."
    say "       (la maquina de podman se maneja por socket local: no hay"
    say "        puerto TCP al que abrir un tunel desde el Mac)."
    say "       Para comprobar el visor hace falta el visor de verdad:"
    say "         adb forward tcp:27015 tcp:27015 && bash $0 pico"
    exit 2
fi

say ""
say "=== 3. El visor se conecta y juega ==="
if "$BIN" "127.0.0.1:$PUERTO" "$RCON_PASSWORD"; then
    ok "el visor sabe jugar"
else
    fail "el visor no pudo jugar"
fi

say ""
say "OK: el banco del visor paso."
exit 0
