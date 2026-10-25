#!/bin/bash
# conectar_sin_usb.sh — Conectar con la Pico por RED y desplegar, sin USB.
#
# POR QUE
#
#   El motor de Factorio solo puede arrancarlo el SHELL (usuario shell), en
#   /data/local/tmp: la app no puede ejecutar un ELF desde su sandbox
#   (execv -> Permission denied) ni leer /data/local/tmp (SELinux).  Sin un
#   shell, el motor no arranca.  Y un shell sin ADB es Termux, que no
#   queremos.
#
#   La salida: la Pico 4 Ultra lleva Android 14 y por lo tanto tiene
#   DEPURACION INALAMBRICA (adb over Wi-Fi).  Con ella hay un shell remoto
#   sin USB y sin Termux.
#
# USO
#
#   1. En la Pico: Ajustes > Sistema > Opciones de desarrollador >
#      "Depuracion inalambrica" (activar).  Aparece una IP:puerto en
#      pantalla (cambia en cada sesion).
#   2. En el Mac:
#          bash scripts/conectar_sin_usb.sh                 # busca solo
#          bash scripts/conectar_sin_usb.sh 192.168.1.50:5555 # con la IP
#          bash scripts/conectar_sin_usb.sh --desplegar     # y despliega
#
# Si no se pasa IP:puerto, se busca en la red: se hace un barrido de la
# subred del Mac probando los puertos de depuracion inalambrica (5555,
# 5037 y el rango aleatorio que usa Android, 37000-44000).

set -uo pipefail

ADB="${ADB:-$HOME/Library/Android/sdk/platform-tools/adb}"
DESTINO=""
DESPLEGAR=0

while [ $# -gt 0 ]; do
    case "$1" in
        --desplegar) DESPLEGAR=1; shift ;;
        --help|-h) sed -n '2,30p' "$0"; exit 0 ;;
        *) DESTINO="$1"; shift ;;
    esac
done

say() { printf '%s\n' "$*"; }

"$ADB" start-server >/dev/null 2>&1 || true

# --- 1. Conectar -----------------------------------------------------------
conectar() {
    local ip="$1"
    for p in 5555 5037 37000 38000 40000 41000 42000 43000 44000; do
        "$ADB" connect "$ip:$p" >/dev/null 2>&1
        if "$ADB" devices | grep -q "^$ip:$p.*device$"; then
            say "conectado: $ip:$p"
            return 0
        fi
    done
    return 1
}

if [ -n "$DESTINO" ]; then
    conectar "$DESTINO" || { say "no se pudo conectar a $DESTINO"; exit 1; }
else
    say "Buscando la Pico en la red (puede tardar un par de minutos)..."
    RED=$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null)
    if [ -z "$RED" ]; then
        say "no se pudo Averiguar la IP local"
        exit 1
    fi
    PREFIJO="${RED%.*}"
    say "subred: $PREFIJO.0/24"
    LISTA=$(for i in $(seq 1 254); do echo "$PREFIJO.$i"; done)
    HECHO=0
    for ip in $LISTA; do
        if conectar "$ip"; then HECHO=1; break; fi
    done
    if [ "$HECHO" != "1" ]; then
        say ""
        say "no se encontro ninguna Pico con depuracion inalambrica."
        say "Comprueba que la tenga activada y que esten en la misma red."
        exit 1
    fi
fi

# --- 2. Desplegar ----------------------------------------------------------
if [ "$DESPLEGAR" = "1" ]; then
    RAIZ="$(cd "$(dirname "$0")/.." && pwd)"
    say "desplegando..."
    (cd "$RAIZ" && bash scripts/deploy_pico.sh)
else
    say ""
    say "Conectado. Para desplegar: bash scripts/conectar_sin_usb.sh --desplegar"
fi
