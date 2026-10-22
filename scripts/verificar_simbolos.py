#!/usr/bin/env python3
"""verificar_simbolos.py — Comprueba que los hooks que necesita el mod 3D
existen en el binario ARM64 de Factorio.

El mod 3D de GDani31 (factorio-3d-mod) engancha el motor por el NOMBRE del
simbolo, pero en Windows el motor compila con MSVC (mangled `?draw@Furnace@@`)
y en Linux con GCC/Clang (mangled Itanium `_ZN6Furnace4drawE...`).  El
nombre legible es el mismo, asi que la pregunta es: ¿estan esos metodos en el
binario ELF ARM64?

Este script lee la tabla de simbolos (.symtab) del ELF y dice, para cada
metodo que el mod 3D necesita, si existe y en que direccion.

Uso: verificar_simbolos.py [ruta/al/factorio]
"""

import struct
import sys

# Metodos que el mod 3D engancha (factorio_3d_models/src/offsets.rs).
HOOKS = [
    "GameRenderer::render",
    "GameRenderer::acknowledgeResize",
    "GameView::createRenderParameters",
    "GameView::getMapPosition",
    "RenderParameters::centerOn",
    "PlayerInputSource::computeWalkingDirectionFromPlayerInputs",
    "AssemblingMachine::draw",
    "Furnace::draw",
    "MiningDrill::draw",
    "Turret::draw",
    "AmmoTurret::draw",
    "WorkingVisualisations::drawCraftingMachine",
    "WorkingVisualisations::drawMiningDrill",
    "Entity::~Entity",
    "Entity::getPrototype",
    "Entity::getSurface",
    "DayTime::getDarkness",
]

STT_FUNC = 2


def leer_simbolos(ruta):
    """Devuelve {nombre_itanium: direccion} de la tabla de simbolos."""
    with open(ruta, "rb") as f:
        datos = f.read()

    if datos[:4] != b"\x7fELF":
        raise SystemExit("no es un ELF")
    es64 = datos[4] == 2
    little = datos[5] == 1
    endian = "<" if little else ">"

    if not es64:
        raise SystemExit("solo ELF64")

    e_shoff = struct.unpack_from(endian + "Q", datos, 0x28)[0]
    e_shentsize = struct.unpack_from(endian + "H", datos, 0x3A)[0]
    e_shnum = struct.unpack_from(endian + "H", datos, 0x3C)[0]
    e_shstrndx = struct.unpack_from(endian + "H", datos, 0x3E)[0]

    secciones = []
    for i in range(e_shnum):
        off = e_shoff + i * e_shentsize
        (sh_name, sh_type, sh_flags, sh_addr, sh_offset, sh_size, sh_link,
         sh_info, sh_align, sh_entsize) = struct.unpack_from(
            endian + "IIQQQQIIQQ", datos, off)
        secciones.append({
            "name_off": sh_name, "type": sh_type, "offset": sh_offset,
            "size": sh_size, "link": sh_link, "entsize": sh_entsize,
            "addr": sh_addr,
        })

    # Nombres de las secciones (para .strtab)
    strtab = None
    for s in secciones:
        if s["type"] == 3:  # SHT_STRTAB
            strtab = s
            break

    def nombre_en(offset, tabla):
        if tabla is None:
            return ""
        fin = datos.find(b"\x00", tabla["offset"] + offset)
        return datos[tabla["offset"] + offset:fin].decode("utf-8", "replace")

    # .symtab y su .strtab
    simbolos = {}
    for s in secciones:
        if s["type"] != 2:  # SHT_SYMTAB
            continue
        strtab_seccion = secciones[s["link"]]
        n = s["size"] // s["entsize"]
        for i in range(n):
            off = s["offset"] + i * s["entsize"]
            st_name, st_info, st_other, st_shndx, st_value, st_size = struct.unpack_from(
                endian + "IBBHQQ", datos, off)
            if st_info & 0xF != STT_FUNC:
                continue
            nombre = nombre_en(st_name, strtab_seccion)
            if nombre:
                simbolos.setdefault(nombre, st_value)
        break
    return simbolos


def partir_mangled(nombre):
    """Extrae (clase, metodo) de un simbolo mangled de Itanium.

    `_ZN[K]<len><clase><len><metodo>...`  ->  ("Furnace", "draw")
    `_ZN5EntityD1Ev`                      ->  ("Entity", "D1")  (destructor)

    Parsear el mangling de verdad es lo que hace la busqueda fiable: buscar
    "Entity" y "getPrototype" a pelo encuentra `MineEntityTechnologyTrigger::
    getPrototype`, que NO es el hook que buscamos.
    """
    if not nombre.startswith("_ZN"):
        return None
    i = 3
    if i < len(nombre) and nombre[i] == "K":
        i += 1

    def tomar_longitud(pos):
        j = pos
        while j < len(nombre) and nombre[j].isdigit():
            j += 1
        if j == pos:
            return None, pos
        return int(nombre[pos:j]), j

    largo, j = tomar_longitud(i)
    if largo is None:
        return None
    clase = nombre[j:j + largo]
    j += largo
    # Destructor: la "longitud" del metodo no lleva digitos, es D0/D1/D2.
    if nombre[j:j + 2] in ("D0", "D1", "D2"):
        return clase, nombre[j:j + 2]
    largo2, k = tomar_longitud(j)
    if largo2 is None:
        return None
    metodo = nombre[k:k + largo2]
    return clase, metodo


def main():
    ruta = sys.argv[1] if len(sys.argv) > 1 else \
        "factorio_linux_arm64/factorio/bin/arm64/factorio"
    print(f"leyendo simbolos de {ruta} ...")
    simbolos = leer_simbolos(ruta)
    print(f"simbolos de funcion encontrados: {len(simbolos)}\n")

    # Indice por (clase, metodo) para buscar EXACTAMENTE.
    indice = {}
    for nombre in simbolos:
        partes = partir_mangled(nombre)
        if partes:
            indice.setdefault(partes, []).append(nombre)

    faltan = []
    for hook in HOOKS:
        clase, _, metodo = hook.partition("::")
        # El destructor de Entity en Itanium es D0/D1/D2, no "~Entity".
        candidatos = [metodo]
        if metodo == "~Entity":
            candidatos = ["D0", "D1", "D2"]
        encontrados = []
        for cand in candidatos:
            encontrados = indice.get((clase, cand), [])
            if encontrados:
                break
        if encontrados:
            encontrados.sort(key=len)
            print(f"  OK       {hook:55} -> {encontrados[0][:78]}")
        else:
            # 1) El metodo puede existir en OTRA clase (la build lo movio).
            en_otras = sorted(n for (c, m), ns in indice.items()
                              if m == metodo and c != clase for n in ns)
            # 2) Puede haberse renombrado dentro de la misma clase.
            METODOS = sorted({m for (c, m) in indice if c == clase})
            faltan.append(hook)
            print(f"  FALTA    {hook}")
            if en_otras:
                print(f"             el metodo SI existe en otra clase: {en_otras[0][:70]}")
            if METODOS:
                print(f"             metodos de {clase}: {', '.join(METODOS[:12])}")

    print()
    if faltan:
        print(f"FALTAN {len(faltan)} de {len(HOOKS)} hooks:")
        for f in faltan:
            print(f"  - {f}")
        return 1
    print(f"Todos los hooks ({len(HOOKS)}) estan en el binario ARM64.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
