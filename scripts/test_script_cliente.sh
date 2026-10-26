#!/usr/bin/env bash
set -uo pipefail

# test_script_cliente.sh — Ejecuta el script de arranque del CLIENTE completo
# contra un arbol de ficheros FALSO y comprueba que hace lo que debe.
#
# Por que: el script del cliente solo se puede ejecutar de verdad en el
# visor, y el cable USB del visor se suelta cada dos por tres.  Aqui se
# monta un arbol falso (payload de Mesa, motor y linker de mentira) y se
# comprueba lo que el script deja en el disco:
#
#   1. El motor arranca con --mp-connect al servidor y con SU config.ini.
#   2. El cliente tiene la lista canonica de mods (base + puente, sin DLC).
#   3. El mod puente del cliente es IDENTICO al del servidor (byte a byte).
#   4. Las expansiones del read-data se apartan a _sin_licencia.
#   5. La FIFO de la consola se crea en el write-data del cliente.
#   6. La consola del motor se pasa como fd 0 (sin EOF).
#
# Codigos de salida:
#   0  todo correcto
#   1  fallo real
#   2  no validable

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${PROJECT_ROOT}/scripts/start_factorio_client.sh"

FALLOS=0
fallo() { echo "  FALLO: $*"; FALLOS=$((FALLOS + 1)); }
ok()    { echo "  ok: $*"; }

if [[ ! -f "$SCRIPT" ]]; then
    echo "FALLO: no existe $SCRIPT"
    exit 1
fi

TMP="$(mktemp -d /tmp/xfv-cli-test.XXXXXX)"
trap 'python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1], ignore_errors=True)" "$TMP"' EXIT

# --- El arbol falso ---------------------------------------------------------
MESA="$TMP/mesa"
MESA_LIB="$MESA/usr/lib/aarch64-linux-gnu"
mkdir -p "$MESA_LIB/dri" "$MESA/glvnd_egl_vendor"
touch "$MESA_LIB/dri/zink_dri.so"
touch "$MESA_LIB/libEGL_mesa.so.0" "$MESA_LIB/libvulkan_freedreno.so"

GAME="$TMP/factorio"
mkdir -p "$GAME/bin/arm64" "$GAME/data/space-age" "$GAME/data/quality"
echo "-- expansion falsa" > "$GAME/data/space-age/control.lua"
echo "-- expansion falsa" > "$GAME/data/quality/control.lua"
# El "motor" falso: imprime sus argumentos y sale.
cat > "$GAME/bin/arm64/factorio" <<'EOF'
#!/bin/sh
echo "MOTOR-FAKE $*" > "$XFV_TEST_ARGS_FILE"
exit 0
EOF
chmod +x "$GAME/bin/arm64/factorio"

GLIBC="$TMP/glibc"
mkdir -p "$GLIBC/lib"
cat > "$GLIBC/lib/ld-linux-aarch64.so.1" <<'EOF'
#!/bin/sh
# El linker falso: se salta el --library-path y ejecuta el motor.
while [ "$1" = "--library-path" ]; do shift 2; done
exec "$@"
EOF
chmod +x "$GLIBC/lib/ld-linux-aarch64.so.1"

# El mod puente en el arbol del servidor (el origen de la verdad).
SERVER_MODS="$TMP/srv/mods"
mkdir -p "$SERVER_MODS/xfactorvr_bridge_0.2.0"
echo "-- control.lua del puente (servidor)" > "$SERVER_MODS/xfactorvr_bridge_0.2.0/control.lua"
echo '{"name":"xfactorvr_bridge","version":"0.2.0"}' > "$SERVER_MODS/xfactorvr_bridge_0.2.0/info.json"

MOD_LIST_CANON="$TMP/mod-list.json"
cp "${PROJECT_ROOT}/scripts/mod-list.json" "$MOD_LIST_CANON"

CLIENT_DIR="$TMP/cliente"
ARGS_FILE="$TMP/args.txt"
export XFV_TEST_ARGS_FILE="$ARGS_FILE"

echo "== 1. El arranque =="
SALIDA="$(
    cd "$TMP" && env \
        FACTORIO_DIR="$GAME" \
        MESA_DIR="$MESA" \
        GLIBC_DIR="$GLIBC" \
        CLIENT_DIR="$CLIENT_DIR" \
        GAME_DIR="$GAME" \
        SERVER_MOD_DIR="$SERVER_MODS" \
        MOD_LIST_CANON="$MOD_LIST_CANON" \
        SERVER_ADDR="127.0.0.1" \
        XFACTORVR_RESOLVER="/no/existe.so" \
        INJECTOR="/no/existe.so" \
        sh "$SCRIPT" 2>&1
)"
RC=$?

if [[ $RC -eq 0 ]]; then
    ok "el script sale con 0"
else
    fallo "el script sale con $RC"
    printf '%s\n' "$SALIDA" | sed 's/^/    /' | tail -12
fi

if [[ -f "$ARGS_FILE" ]]; then
    ARGS="$(cat "$ARGS_FILE")"
    ok "el motor arranca: ${ARGS:0:110}"
else
    fallo "el motor no llegó a arrancar"
    ARGS=""
fi

case "$ARGS" in
    *--mp-connect\ 127.0.0.1:34197*) ok "conecta al servidor con --mp-connect" ;;
    *) fallo "no conecta con --mp-connect (args: $ARGS)" ;;
esac
case "$ARGS" in
    *--config\ "$CLIENT_DIR/config/config.ini"*) ok "usa SU propio config.ini" ;;
    *) fallo "no usa el config.ini del cliente" ;;
esac
case "$ARGS" in
    *"<&3"*) ok "el stdin del motor es la FIFO (sin EOF)" ;;
    *) fallo "el motor no recibe la consola por fd 0" ;;
esac

echo ""
echo "== 2. Lista de mods del cliente =="
CLIENT_MOD_LIST="$CLIENT_DIR/mods/mod-list.json"
if [[ -f "$CLIENT_MOD_LIST" ]]; then
    if diff -q "$MOD_LIST_CANON" "$CLIENT_MOD_LIST" >/dev/null 2>&1; then
        ok "la lista de mods es la canonica"
    else
        fallo "la lista de mods no es la canonica"
        diff "$MOD_LIST_CANON" "$CLIENT_MOD_LIST" | sed 's/^/    /'
    fi
else
    fallo "no se escribio la lista de mods del cliente"
fi

echo ""
echo "== 3. El mod puente es identico al del servidor =="
C_CLIENT="$CLIENT_DIR/mods/xfactorvr_bridge_0.2.0/control.lua"
C_SERVIDOR="$SERVER_MODS/xfactorvr_bridge_0.2.0/control.lua"
if [[ -f "$C_CLIENT" ]]; then
    if cmp -s "$C_CLIENT" "$C_SERVIDOR"; then
        ok "control.lua identico byte a byte"
    else
        fallo "el control.lua del cliente difiere del servidor"
    fi
else
    fallo "no se copio el mod puente al cliente"
fi

echo ""
echo "== 4. Las expansiones se apartan del read-data =="
for exp in space-age quality; do
    if [[ -d "$GAME/data/_sin_licencia/$exp" ]]; then
        ok "$exp esta en _sin_licencia"
    else
        fallo "$exp sigue en el read-data (el cliente grafico lo rechaza)"
    fi
    if [[ -d "$GAME/data/$exp" ]]; then
        fallo "$exp sigue en el read-data"
    fi
done

echo ""
echo "== 5. La FIFO de la consola =="
if [[ -p "$CLIENT_DIR/consola.fifo" ]]; then
    ok "la FIFO esta en el write-data del cliente"
else
    fallo "no se creo la FIFO en $CLIENT_DIR"
fi

echo ""
if [[ $FALLOS -eq 0 ]]; then
    echo "RESULTADO: OK (0)"
    exit 0
fi
echo "RESULTADO: FALLO ($FALLOS problemas)"
exit 1
