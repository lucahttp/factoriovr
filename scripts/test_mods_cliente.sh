#!/usr/bin/env bash
set -uo pipefail

# test_mods_cliente.sh — Comprueba las invariantes que hacen que el cliente
# GRAFICO se pueda conectar al servidor headless.
#
# Lo que se comprueba (todo fallo real, medido en el visor Pico 4 Ultra):
#
#   1. La lista canonica de mods existe, es JSON valido y NO lleva expansiones.
#   2. El servidor y el cliente usan ESA MISMA lista.
#   3. El cliente aparta las expansiones de su read-data.
#   4. La FIFO de la consola vive en el write-data del cliente (escribible).
#   5. El servidor detecta un motor ya arrancado por cmdline, no por comm.
#   6. Los dos scripts son POSIX validos (sh -n).
#
# Por que las expansiones se quitan: el cliente GRAFICO comprueba la licencia
# de expansion y, sin cuenta de Factorio, se niega a cargarlas --
#   "Failed to load mods: Mod elevated-rails requires Space Age expansion."
# El servidor headless no hace ese chequeo: con expansiones el servidor
# carga 6 mods y el cliente rechaza 2, y nunca se conectan.
#
# Codigos de salida:
#   0  todo correcto
#   1  fallo real
#   2  no validable (falta python3 para el JSON)

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MOD_LIST="${PROJECT_ROOT}/scripts/mod-list.json"
SRV="${PROJECT_ROOT}/scripts/start_factorio_standalone.sh"
CLI="${PROJECT_ROOT}/scripts/start_factorio_client.sh"

FALLOS=0

fallo() { echo "FALLO: $*"; FALLOS=$((FALLOS + 1)); }
ok()    { echo "  ok: $*"; }

for f in "$MOD_LIST" "$SRV" "$CLI"; do
    if [[ ! -f "$f" ]]; then
        echo "FALLO: no existe $f"
        exit 1
    fi
done

echo "== 1. Lista canonica de mods =="
if ! command -v python3 >/dev/null 2>&1; then
    echo "NO VALIDABLE: no hay python3 para leer el JSON"
    exit 2
fi

python3 - "$MOD_LIST" <<'PY' || FALLOS=$((FALLOS + 1))
import json, sys

ruta = sys.argv[1]
try:
    with open(ruta) as f:
        datos = json.load(f)
except Exception as exc:
    print(f"  FALLO: {ruta} no es JSON valido: {exc}")
    sys.exit(1)

mods = {m["name"]: m.get("enabled", False) for m in datos.get("mods", [])}

EXPANSIONES = {"space-age", "quality", "recycler", "elevated-rails"}

problemas = []

if not mods.get("base"):
    problemas.append("falta 'base' activado")
if not mods.get("xfactorvr_bridge"):
    problemas.append("falta 'xfactorvr_bridge' activado")

presentes = EXPANSIONES & set(mods)
if presentes:
    problemas.append("lleva expansiones: " + ", ".join(sorted(presentes)))

if problemas:
    for p in problemas:
        print(f"  FALLO: {p}")
    sys.exit(1)

print(f"  ok: base + xfactorvr_bridge, sin expansiones ({len(mods)} mods)")
PY

echo ""
echo "== 2. Servidor y cliente usan la misma lista =="
if grep -q 'MOD_LIST_CANON' "$SRV" && grep -q 'mod-list.json' "$SRV"; then
    ok "el servidor copia la lista canonica a MOD_DIR/mod-list.json"
else
    fallo "el servidor no copia la lista canonica (MOD_LIST_CANON)"
fi
if grep -q 'MOD_LIST_CANON' "$CLI" && grep -q 'CLIENT_MODS' "$CLI"; then
    ok "el cliente copia la lista canonica a su carpeta de mods"
else
    fallo "el cliente no copia la lista canonica (MOD_LIST_CANON)"
fi
if grep -q 'SERVER_MOD_DIR' "$CLI"; then
    ok "el cliente copia el mod puente desde el arbol del servidor"
else
    fallo "el cliente no copia el mod puente del servidor"
fi

echo ""
echo "== 3. El cliente aparta las expansiones de su read-data =="
if grep -q '_sin_licencia' "$CLI" && grep -q 'space-age' "$CLI"; then
    ok "las expansiones se mueven a <read-data>/_sin_licencia"
else
    fallo "el cliente no aparta las expansiones del read-data"
fi

echo ""
echo "== 4. La FIFO de la consola esta en el write-data del cliente =="
# El write-data del servidor (/data/local/tmp/factorio) es de root: la FIFO
# ahi no se puede crear y el canal de ordenes sin RCON se queda sin usar.
if grep -q 'CONSOLA_FIFO:-\$CLIENT_DIR/consola.fifo' "$CLI"; then
    ok "la FIFO cuelga de \$CLIENT_DIR"
else
    fallo "la FIFO no cuelga de \$CLIENT_DIR (no se podria crear)"
fi

echo ""
echo "== 5. El servidor detecta un motor ya vivo =="
# En el visor el proceso del motor se llama "ld-linux-aarch64.so.1", no
# "factorio": con comm == "factorio" la deteccion nunca daba y se lanzaba un
# segundo motor que moria con "Host address is already in use".
if grep -q 'bin/arm64/factorio' "$SRV" && grep -q 'cmdline' "$SRV"; then
    ok "la deteccion mira el cmdline (bin/arm64/factorio)"
else
    fallo "la deteccion de proceso vivo no usa el cmdline"
fi
if grep -q 'Host address is already in use\|already in use' "$SRV"; then
    ok "el script avisa del conflicto de puerto"
else
    fallo "el script no contempla el conflicto de puerto"
fi

echo ""
echo "== 6. Sintaxis POSIX de los dos scripts =="
for s in "$SRV" "$CLI"; do
    if sh -n "$s" 2>/dev/null; then
        ok "$(basename "$s") compila (sh -n)"
    else
        fallo "$(basename "$s") tiene un error de sintaxis"
        sh -n "$s" 2>&1 | head -5
    fi
done

echo ""
if [[ $FALLOS -eq 0 ]]; then
    echo "RESULTADO: OK (0)"
    exit 0
fi
echo "RESULTADO: FALLO ($FALLOS problemas)"
exit 1
