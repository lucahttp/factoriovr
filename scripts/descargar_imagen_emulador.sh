#!/usr/bin/env bash
set -uo pipefail

# descargar_imagen_emulador.sh — Baja la imagen del sistema del emulador
# (aosp_atd arm64-v8a) con N conexiones en paralelo.
#
# Por que no sdkmanager: medido en esta maquina, dl.google.com con HTTP/2
# entrega ~250 KB/s y con HTTP/1.1 ~400 KB/s.  La imagen son 533 MB, asi que
# con una sola conexion son ~22 minutos.  Con 4 conexiones en paralelo por
# rangos el tiempo baja a la mitad, y ademas cada parte se puede reanudar
# con -C - si la conexion se cae.
#
# Al final se verifica el tamano exacto que declara el servidor y se deja
# el zip listo para descomprimir en system-images/.
#
# Uso: bash scripts/descargar_imagen_emulador.sh [partes]

DEST="${DEST:-/Volumes/X10Pro/Android/sdk/.temp/descarga}"
URL="${URL:-https://dl.google.com/android/repository/sys-img/aosp_atd/arm64-v8a-33_r02.zip}"
ZIP="$DEST/arm64-v8a-33_r02.zip"
PARTES="${1:-4}"
TOTAL=559254461

mkdir -p "$DEST"

echo "=== Descargando la imagen ATD arm64-v8a ($TOTAL bytes) en $PARTES partes ==="

LONGITUD=$(( (TOTAL + PARTES - 1) / PARTES ))
PIDS=()
for i in $(seq 0 $((PARTES - 1))); do
    INICIO=$(( i * LONGITUD ))
    FIN=$(( INICIO + LONGITUD - 1 ))
    if [ "$FIN" -ge "$TOTAL" ]; then FIN=$(( TOTAL - 1 )); fi
    if [ "$INICIO" -gt "$FIN" ]; then continue; fi
    PARTE="$DEST/parte_$i"
    (
        for intento in 1 2 3 4 5 6 7 8 9 10; do
            curl -s --http1.1 --retry 5 --retry-delay 3 --retry-all-errors \
                 -r "$INICIO-$FIN" -C - -o "$PARTE" "$URL" && break
            echo "  parte $i: reintento $intento"
            sleep 3
        done
    ) &
    PIDS+=($!)
done

for p in "${PIDS[@]}"; do wait "$p"; done

echo "=== Juntando las partes ==="
cat "$DEST"/parte_* > "$ZIP"
rm -f "$DEST"/parte_*

TAM=$(stat -f%z "$ZIP")
if [ "$TAM" != "$TOTAL" ]; then
    echo "ERROR: tamano $TAM != $TOTAL esperado" >&2
    exit 1
fi
echo "OK: $ZIP ($TAM bytes)"
shasum -a 256 "$ZIP"
