#!/usr/bin/env python3
"""resolve_hooks.py — Resuelve los hooks del mod 3D por nombre de símbolo.

El factorio-3d-mod de GDani31 engancha el pipeline de render de Factorio
resolviendo las funciones por nombre de símbolo del PDB de Windows, con RVAs
de reserva por si el símbolo desaparece.

En Linux ARM64 no hay PDB: el binario del motor viene SIN STRIPEAR y con
148.802 símbolos C++ con nombre, así que se resuelve directamente del symtab
del ELF. Este script hace esa resolución y produce la tabla de direcciones
que usaría el .so del port.

Uso:
    python3 scripts/resolve_hooks.py --elf <factorio.elf> [--json salida.json]

Los nombres se buscan por fragmento de texto (mangled o des-mangled), igual que
hace el upstream con el PDB, para que un cambio de version no rompa el port.
"""

import argparse
import json
import os
import re
import subprocess
import sys

# Hooks del mod 3D, con el nombre de la version 2.0.77 (Windows) y el
# equivalente real en 2.1.22 (Linux ARM64). Los fragmentos se buscan en el
# simbolo des-mangled; se aceptan varios por si el nombre cambia de forma.
HOOKS = {
    # --- pipeline de render -------------------------------------------------
    "GAME_RENDERER_RENDER": [
        "GameRenderer::render(",
    ],
    "GAME_RENDERER_PREPARE": [
        "GameRenderer::prepare(",
    ],
    "GAME_RENDERER_RENDER_INTERNAL": [
        "GameRenderer::renderInternal(",
    ],
    "RP_CENTER_ON": [
        "RenderParameters::centerOn(",
    ],
    # --- camara --------------------------------------------------------------
    "GET_MAP_POSITION": [
        "GameView::getMapPosition(",
    ],
    "GAME_VIEW_CALCULATE_SURFACE_VIEW": [
        "GameView::calculateSurfaceView(",
    ],
    # --- entidades reemplazadas por modelos 3D ------------------------------
    "ASSEMBLING_MACHINE_DRAW": [
        "AssemblingMachine::draw(",
    ],
    "FURNACE_DRAW": [
        "Furnace::draw(",
    ],
    "MINING_DRILL_DRAW": [
        "MiningDrill::draw(",
    ],
    "TURRET_DRAW": [
        "Turret::draw(",
    ],
    "AMMO_TURRET_DRAW": [
        "AmmoTurret::draw(",
    ],
    # --- estado de las maquinas (lo que necesita el modelo 3D) --------------
    "CRAFTING_MACHINE_DRAW": [
        "CraftingMachine::draw(",
    ],
    "WORKING_VISUALISATION_DRAW": [
        "WorkingVisualisation::draw(",
    ],
    # --- items sobre las cintas ---------------------------------------------
    "DRAW_ITEM_STACK": [
        "ItemStackPainter::drawItemStack(",
    ],
    "TB_DRAW_ITEMS": [
        "TransportBelt::drawItems(",
    ],
    "DRAW_SCALED_TINTED": [
        "DrawQueue::drawScaledTinted(",
    ],
    # --- consultas (se llaman, no se enganchan) -----------------------------
    "ENTITY_GET_PROTOTYPE": [
        "Entity::getPrototype(",
    ],
    "ENTITY_GET_SURFACE": [
        "Entity::getSurface(",
    ],
    "DAYTIME_SET_POSITION": [
        "DayTime::setPosition(",
    ],
    "PLAYER_INPUT_WALK_DIR": [
        "PlayerInputSource::computeWalkingDirectionFromPlayerInputs(",
    ],
}


def simbolos(elf):
    """Simbolos definidos del ELF, des-mangled, como (nombre, direccion)."""
    if not os.path.exists(elf):
        print(f"No existe el binario: {elf}", file=sys.stderr)
        return []
    try:
        out = subprocess.run(
            ["nm", "-C", "--defined-only", elf],
            capture_output=True, text=True, check=False, timeout=600,
        ).stdout
    except (OSError, subprocess.TimeoutExpired) as exc:
        print(f"nm fallo: {exc}", file=sys.stderr)
        return []
    res = []
    for linea in out.splitlines():
        partes = linea.split(" ", 2)
        if len(partes) < 3:
            continue
        addr, tipo, nombre = partes[0], partes[1], partes[2]
        if tipo.lower() not in ("t", "w"):
            continue
        try:
            res.append((int(addr, 16), nombre.strip()))
        except ValueError:
            continue
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--elf", required=True, help="binario ELF de Factorio")
    ap.add_argument("--json", help="fichero donde escribir la tabla")
    ap.add_argument("--min", type=int, default=1,
                    help="numero minimo de simbolos que deben casar para dar el hook por bueno")
    args = ap.parse_args()

    sims = simbolos(args.elf)
    if not sims:
        print("No se pudieron leer los simbolos del ELF.", file=sys.stderr)
        return 1
    print(f"Simbolos leidos: {len(sims)}")

    tabla = {}
    for hook, fragmentos in HOOKS.items():
        encontrados = []
        for frag in fragmentos:
            for addr, nombre in sims:
                if frag in nombre:
                    encontrados.append((addr, nombre))
                    break
        estado = "OK" if len(encontrados) >= args.min else "AUSENTE"
        tabla[hook] = {
            "estado": estado,
            "simbolos": [{"addr": f"0x{a:x}", "nombre": n} for a, n in encontrados],
        }
        linea = f"  {estado:8s} {hook:32s}"
        if encontrados:
            linea += f" 0x{encontrados[0][0]:x}  {encontrados[0][1][:90]}"
        print(linea)

    faltan = [k for k, v in tabla.items() if v["estado"] != "OK"]
    print()
    print(f"Hooks resueltos: {len(HOOKS) - len(faltan)}/{len(HOOKS)}")
    if faltan:
        print("Sin resolver: " + ", ".join(faltan))
    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(tabla, fh, indent=2, ensure_ascii=False)
        print(f"Tabla escrita en {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
