#!/usr/bin/env python3
"""resolve_hooks.py — Resuelve los hooks del mod 3D por nombre de símbolo.

El factorio-3d-mod de GDani31 engancha el pipeline de render de Factorio
resolviendo las funciones por nombre de símbolo del PDB de Windows, con RVAs
de reserva por si el símbolo desaparece.

En Linux ARM64 no hay PDB: el binario del motor viene SIN STRIPEAR y con
148.802 símbolos C++ con nombre, así que se resuelve directamente del symtab
del ELF. Este script hace esa resolución y produce la tabla de direcciones
que usaría el .so del port.

Cambios respecto a la versión 2.0.77 (Windows) del upstream:

  * La búsqueda es EXACTA sobre el nombre des-mangled y exige que el nombre
    de la clase no esté pegado a otro ("Turret::draw" ya no se confunde con
    "AmmoTurret::draw").
  * `WorkingVisualisation::draw` ya no existe: en 2.1.22 el dibujado de la
    visualizacion de trabajo vive en `WorkingVisualisationDrawState::draw`
    (y se llama desde `CraftingMachine::draw`).
  * `Entity::getPrototype()` y `Entity::getSurface()` ya no son funciones: en
    2.1.22 son accesos a miembro inlinados por el compilador. Se resuelven
    como LECTURA DE CAMPO: el puntero al prototipo esta en Entity+0x48 y el
    puntero a la superficie en Entity+0x70. El script VERIFICA esos offsets
    desensamblando la funcion que los usa.

Uso:
    python3 scripts/resolve_hooks.py --elf <factorio.elf> [--json salida.json]

Los nombres se buscan por fragmento de texto sobre el símbolo des-mangled,
igual que hace el upstream con el PDB, para que un cambio de version no
rompa el port.
"""

import argparse
import json
import os
import re
import struct
import subprocess
import sys

# ---------------------------------------------------------------------------
# Tabla de hooks.
#
# Cada entrada es un dict con:
#   "simbolo": fragmentos del nombre des-mangled (se busca el primero que case)
#   "nota":    comentario de por que ese nombre en 2.1.22
#   "campo":   (opcional) si en vez de una funcion es un miembro:
#              {"offset": 0x48, "clase": "Entity", "referencia": "simbolo",
#               "descripcion": "..."}
# ---------------------------------------------------------------------------

HOOKS = {
    # --- pipeline de render -------------------------------------------------
    "GAME_RENDERER_RENDER": {
        "simbolo": ["GameRenderer::render("],
        "nota": "el dibujado de un frame completo",
    },
    "GAME_RENDERER_PREPARE": {
        "simbolo": ["GameRenderer::prepare("],
        "nota": "prepara los recursos antes de dibujar",
    },
    "GAME_RENDERER_RENDER_INTERNAL": {
        "simbolo": ["GameRenderer::renderInternal(bool)::$_0>::_M_invoke"],
        "nota": ("en 2.1.22 GameRenderer::renderInternal(bool) ya no es una "
                 "funcion out-of-line: su cuerpo quedo outlining en la lambda "
                 "que recibe el DrawQueue"),
    },
    "RP_CENTER_ON": {
        "simbolo": ["RenderParameters::centerOn("],
        "nota": "centerOn escribe la escala real (displayScale x zoom)",
    },
    # --- camara --------------------------------------------------------------
    "GET_MAP_POSITION": {
        "simbolo": ["GameView::getMapPosition("],
        "nota": "posicion del mapa en pixeles",
    },
    "GAME_VIEW_CALCULATE_SURFACE_VIEW": {
        "simbolo": ["GameView::calculateSurfaceView("],
        "nota": "plano de la superficie para la camara",
    },
    # --- entidades reemplazadas por modelos 3D ------------------------------
    "ASSEMBLING_MACHINE_DRAW": {
        "simbolo": ["AssemblingMachine::draw("],
    },
    "FURNACE_DRAW": {
        "simbolo": ["Furnace::draw("],
    },
    "MINING_DRILL_DRAW": {
        "simbolo": ["MiningDrill::draw("],
    },
    "TURRET_DRAW": {
        "simbolo": ["Turret::draw("],
        "nota": "NO es AmmoTurret::draw: la busqueda exige que el nombre de la clase no este pegado",
    },
    "AMMO_TURRET_DRAW": {
        "simbolo": ["AmmoTurret::draw("],
    },
    # --- estado de las maquinas (lo que necesita el modelo 3D) --------------
    "CRAFTING_MACHINE_DRAW": {
        "simbolo": ["CraftingMachine::draw("],
        "nota": ("en 2.1.22 WorkingVisualisation::draw se fusiono dentro de "
                 "CraftingMachine::draw (la sobrecarga que recibe "
                 "WorkingVisualisations const&)"),
    },
    "WORKING_VISUALISATION_DRAW": {
        "simbolo": ["WorkingVisualisationDrawState::draw("],
        "nota": ("2.1.22: la visualizacion de trabajo se dibuja desde "
                 "WorkingVisualisationDrawState::draw (antes WorkingVisualisation::draw)"),
    },
    # --- items sobre las cintas ---------------------------------------------
    "DRAW_ITEM_STACK": {
        "simbolo": ["ItemStackPainter::drawItemStack("],
    },
    "TB_DRAW_ITEMS": {
        "simbolo": ["TransportBelt::drawItems("],
    },
    "DRAW_SCALED_TINTED": {
        "simbolo": ["DrawQueue::drawScaledTinted("],
    },
    # --- consultas (se llaman, no se enganchan) -----------------------------
    "ENTITY_GET_PROTOTYPE": {
        "simbolo": [],
        "campo": {
            "offset": 0x48,
            "clase": "Entity",
            "referencia": "Entity::prototypeCollide",
            "descripcion": ("EntityPrototype const* — en 2.1.22 "
                            "Entity::getPrototype() esta inlinado y solo queda "
                            "el miembro"),
        },
    },
    "ENTITY_GET_SURFACE": {
        "simbolo": [],
        "campo": {
            "offset": 0x70,
            "clase": "Entity",
            "referencia": "Entity::setupPosition",
            "descripcion": ("Surface* — en 2.1.22 Entity::getSurface() esta "
                            "inlinado y solo queda el miembro"),
        },
    },
    "DAYTIME_SET_POSITION": {
        "simbolo": ["DayTime::setPosition("],
    },
    "PLAYER_INPUT_WALK_DIR": {
        "simbolo": ["PlayerInputSource::computeWalkingDirectionFromPlayerInputs("],
    },
}


# ---------------------------------------------------------------------------
# Lectura del ELF
# ---------------------------------------------------------------------------

class Elf:
    """Lo justo del ELF para resolver simbolos y leer bytes de codigo."""

    def __init__(self, path):
        self.path = path
        self.sections = []       # (nombre, addr, offset, size)
        self.symbols = []        # (addr, nombre_desmangled, tipo)
        self._raw = None
        self._leer_simbolos()
        self._leer_secciones()

    def _leer_simbolos(self):
        try:
            out = subprocess.run(
                ["nm", "-C", "--defined-only", self.path],
                capture_output=True, text=True, check=False, timeout=900,
            ).stdout
        except (OSError, subprocess.TimeoutExpired) as exc:
            print(f"nm fallo: {exc}", file=sys.stderr)
            return
        for linea in out.splitlines():
            partes = linea.split(" ", 2)
            if len(partes) < 3:
                continue
            addr, tipo, nombre = partes[0], partes[1], partes[2].strip()
            if tipo.lower() not in ("t", "w"):
                continue
            try:
                self.symbols.append((int(addr, 16), nombre, tipo.lower()))
            except ValueError:
                continue

    def _leer_secciones(self):
        """Secciones del ELF leidas en Python puro (no hace falta readelf)."""
        try:
            with open(self.path, "rb") as fh:
                hdr = fh.read(64)
                if len(hdr) < 64 or hdr[:4] != b"\x7fELF" or hdr[4] != 2:
                    return
                e_shoff = struct.unpack_from("<Q", hdr, 0x28)[0]
                e_shentsize = struct.unpack_from("<H", hdr, 0x3A)[0]
                e_shnum = struct.unpack_from("<H", hdr, 0x3C)[0]
                e_shstrndx = struct.unpack_from("<H", hdr, 0x3E)[0]
                if e_shoff == 0 or e_shnum == 0 or e_shentsize < 64:
                    return
                fh.seek(e_shoff)
                tabla = fh.read(e_shnum * e_shentsize)
                nombres = b""
                if e_shstrndx < e_shnum:
                    off = struct.unpack_from("<Q", tabla, e_shstrndx * e_shentsize + 0x18)[0]
                    size = struct.unpack_from("<Q", tabla, e_shstrndx * e_shentsize + 0x20)[0]
                    fh.seek(off)
                    nombres = fh.read(size)
                for i in range(e_shnum):
                    base = i * e_shentsize
                    sh_name = struct.unpack_from("<I", tabla, base + 0x00)[0]
                    sh_addr = struct.unpack_from("<Q", tabla, base + 0x10)[0]
                    sh_offset = struct.unpack_from("<Q", tabla, base + 0x18)[0]
                    sh_size = struct.unpack_from("<Q", tabla, base + 0x20)[0]
                    fin = nombres.find(b"\x00", sh_name)
                    nombre = nombres[sh_name:fin].decode("ascii", "replace")
                    self.sections.append((nombre, sh_addr, sh_offset, sh_size))
        except (OSError, struct.error) as exc:
            print(f"no se pudieron leer las secciones: {exc}", file=sys.stderr)

    def bytes_en_vaddr(self, vaddr, largo):
        """Bytes del fichero que corresponden a una direccion virtual."""
        for _nombre, addr, offset, size in self.sections:
            if addr and addr <= vaddr < addr + size:
                with open(self.path, "rb") as fh:
                    fh.seek(offset + (vaddr - addr))
                    datos = fh.read(largo)
                return datos
        return None

    def simbolos_con(self, fragmento):
        """Simbolos cuyo nombre des-mangled contiene `fragmento`.

        El fragmento tiene que empezar en un limite de palabra: "Turret::draw("
        no puede casar dentro de "AmmoTurret::draw(".
        """
        patron = re.compile(r"(?<![A-Za-z0-9_])" + re.escape(fragmento))
        return [(a, n) for a, n, _t in self.symbols if patron.search(n)]


def buscar_ldr_imm(elf, simbolo, offset_esperado, x0_reg=0):
    """Comprueba si la funcion `simbolo` toca [x0 + offset_esperado].

    Codificacion AArch64 de "ldr/str Xt, [Xn, #imm12*8]":
        0xF9400000 | (imm12 << 10) | (Rn << 5) | Rt   (lectura)
        0xF9000000 | (imm12 << 10) | (Rn << 5) | Rt   (escritura)
    Se aceptan las dos: hay getters que leen el miembro (getPrototype) y
    otros que lo escriben (setupPosition guardando la superficie).
    """
    for addr, nombre in elf.simbolos_con(simbolo):
        datos = elf.bytes_en_vaddr(addr, 160)
        if not datos:
            continue
        for i in range(0, len(datos) - 3, 4):
            insn = struct.unpack_from("<I", datos, i)[0]
            clase = insn & 0xFFC00000
            if clase not in (0xF9400000, 0xF9000000):
                continue
            imm12 = (insn >> 10) & 0xFFF
            rn = (insn >> 5) & 0x1F
            rt = insn & 0x1F
            if rn == x0_reg and imm12 * 8 == offset_esperado:
                op = "ldr" if clase == 0xF9400000 else "str"
                return True, nombre, addr, i, op, rt
    return False, None, None, None, None, None


# ---------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--elf", required=True, help="binario ELF de Factorio")
    ap.add_argument("--json", help="fichero donde escribir la tabla")
    ap.add_argument("--min", type=int, default=1,
                    help="numero minimo de simbolos que deben casar para dar el hook por bueno")
    args = ap.parse_args()

    if not os.path.exists(args.elf):
        print(f"No existe el binario: {args.elf}", file=sys.stderr)
        return 1

    elf = Elf(args.elf)
    if not elf.symbols:
        print("No se pudieron leer los simbolos del ELF.", file=sys.stderr)
        return 1
    print(f"Simbolos leidos: {len(elf.symbols)}")

    tabla = {}
    for hook, spec in HOOKS.items():
        encontrados = []
        for frag in spec.get("simbolo", []):
            for addr, nombre in elf.simbolos_con(frag):
                encontrados.append((addr, nombre))
            if encontrados:
                break
        estado = "OK" if len(encontrados) >= args.min else "AUSENTE"

        entrada = {
            "estado": estado,
            "simbolos": [{"addr": f"0x{a:x}", "nombre": n} for a, n in encontrados],
        }
        if spec.get("nota"):
            entrada["nota"] = spec["nota"]

        # Hooks que en 2.1.22 son miembros y no funciones: se verifica el
        # offset desensamblando la funcion que lo usa.
        campo = spec.get("campo")
        if campo:
            ok, nombre, addr, off, op, rt = buscar_ldr_imm(
                elf, campo["referencia"], campo["offset"])
            entrada["tipo"] = "campo"
            entrada["campo"] = {
                "clase": campo["clase"],
                "offset": campo["offset"],
                "offset_hex": f"0x{campo['offset']:x}",
                "descripcion": campo["descripcion"],
            }
            if ok:
                entrada["estado"] = "OK"
                entrada["verificacion"] = (
                    f"{nombre} hace '{op} x{rt}, [x0 + 0x{campo['offset']:x}]' "
                    f"(instruccion en 0x{addr + off:x})")
            else:
                entrada["estado"] = "AUSENTE"
                entrada["verificacion"] = (
                    f"no se ha encontrado un acceso a [x0 + 0x{campo['offset']:x}] "
                    f"en {campo['referencia']}")

        tabla[hook] = entrada
        linea = f"  {entrada['estado']:8s} {hook:32s}"
        if encontrados:
            linea += f" 0x{encontrados[0][0]:x}  {encontrados[0][1][:80]}"
        elif campo:
            linea += f" campo {campo['clase']}+0x{campo['offset']:x}"
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
