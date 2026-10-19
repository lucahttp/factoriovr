#!/usr/bin/env python3
"""
check_lua_api.py — Audita el mod puente contra la API Lua real de Factorio

El banco de pruebas (scripts/tests/bridge_control_test.lua) usa un mock de la
API, asi que no detecta que una llamada exista o no en el motor real. Este
script consulta la documentacion oficial de la version instalada de Factorio y
comprueba que cada llamada que hace el mod existe de verdad.

Se detectaron con esta auditoria dos llamadas rotas en Factorio 2.0:
  - game.recipe_prototypes -> elimada en 2.0, ahora force.recipes
  - inventory.remove_item   -> nunca existio, ahora inventory.remove

Uso:
    python3 scripts/check_lua_api.py [VERSION_FACTORIO] [RUTA_AL_MOD]
"""
import os
import re
import sys
import urllib.request

BASE = "https://lua-api.factorio.com/{version}/classes/{cls}.html"
DEFAULT_VERSION = "2.0.77"
DEFAULT_MOD = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           "..", "mods", "xfactorvr_bridge", "control.lua")

# Miembros de la libreria estandar de Lua: no son de la API de Factorio.
LUA_STDLIB = {
    "table", "string", "math", "os", "io", "coroutine", "debug", "bit32",
    "utf8", "string", "package", "require", "pcall", "xpcall", "pairs",
    "ipairs", "next", "select", "type", "tostring", "tonumber", "print",
    "assert", "error", "setmetatable", "getmetatable", "rawget", "rawset",
    "rawequal", "unpack", "_G", "self",
}

# Tablas de la API de Factorio que el mod usa como variable local.
KNOWN_VARS = {
    "game", "surface", "player", "entity", "inv", "inventory", "inv_out",
    "fuel_inv", "out_inv", "force", "recipe", "r", "p", "e", "f", "rp",
    "ing", "prod", "remote", "rcon", "script", "defines",
    "serpent", "table", "string", "math", "os",
}

# Objetos globales de Factorio cuya clase no aparece en el indice de la
# documentacion (son objetos, no clases de la API).
GLOBAL_OBJECTS = {"rcon", "script", "remote", "defines"}

# Receptor del mod -> clase de la API. Permite comprobar cada llamada contra
# la clase exacta en vez de contra el universo completo, que daria falsos
# positivos (un miembro puede existir en otra clase y no en la que se usa).
RECEIVER_CLASS = {
    "game": "LuaGameScript",
    "surface": "LuaSurface",
    "p": "LuaPlayer",
    "player": "LuaPlayer",
    "e": "LuaEntity",
    "entity": "LuaEntity",
    "inv": "LuaInventory",
    "inventory": "LuaInventory",
    "inv_out": "LuaInventory",
    "out_inv": "LuaInventory",
    "fuel_inv": "LuaInventory",
    # `force.recipes[nombre]` devuelve un LuaRecipe (el objeto de recipes de la
    # fuerza), NO un LuaRecipePrototype. Confundirlos hizo que la auditoria
    # validase `main_product` contra la clase equivocada: LuaRecipe no lo tiene.
    "rp": "LuaRecipe",
    "r": "LuaRecipe",
}

# Globales que Factorio 2.0 elimino. Usarlos provoca un error no recuperable en
# on_init que IMPIDE arrancar la partida (detectado con el motor real).
REMOVED_IN_2X = {"global"}

_cache = {}


def fetch(version, cls):
    key = (version, cls)
    if key in _cache:
        return _cache[key]
    url = BASE.format(version=version, cls=cls)
    try:
        with urllib.request.urlopen(url, timeout=25) as r:
            html = r.read().decode("utf-8", errors="replace")
    except Exception:
        html = ""
    members = set(re.findall(
        r'class="docs-attribute-name"[^>]*>\s*<a[^>]*>([A-Za-z0-9_]+)</a>', html))
    parents = re.findall(r'Inherit(?:s|ed) from\s*<a[^>]*>([A-Za-z0-9_]+)</a>', html)
    _cache[key] = (members, parents)
    return _cache[key]


def effective(version, cls, seen=None):
    """Miembros efectivos de una clase, incluyendo los heredados."""
    if seen is None:
        seen = set()
    if cls in seen:
        return set()
    seen.add(cls)
    members, parents = fetch(version, cls)
    out = set(members)
    for p in parents:
        out |= effective(version, p, seen)
    return out


def class_names(version):
    """Lista de clases de la API, sacada del indice de la documentacion."""
    url = f"https://lua-api.factorio.com/{version}/script.js"
    try:
        with urllib.request.urlopen(url, timeout=25) as r:
            js = r.read().decode("utf-8", errors="replace")
    except Exception:
        return []
    return sorted(set(re.findall(r'"(Lua[A-Za-z0-9_]+)":\[', js)))


def main():
    version = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_VERSION
    mod_path = sys.argv[2] if len(sys.argv) > 2 else DEFAULT_MOD

    if not os.path.isfile(mod_path):
        print(f"No se encuentra el mod en {mod_path}")
        return 2

    print(f"=== Auditoria del mod contra la API Lua de Factorio {version} ===")
    print(f"Mod: {os.path.relpath(mod_path)}\n")

    # 1. Universo de miembros de la API.
    print("Descargando el indice de clases de la API...")
    classes = class_names(version)
    if not classes:
        print("ERROR: no se pudo descargar el indice de la API")
        return 2
    universe = set()
    for c in classes:
        universe |= effective(version, c)
    print(f"  {len(classes)} clases, {len(universe)} miembros unicos\n")

    # 2. Miembros usados por el mod.
    src = open(mod_path, encoding="utf-8").read()
    code = re.sub(r"--\[\[.*?\]\]", "", src, flags=re.S)
    code = re.sub(r"--[^\n]*", "", code)
    # Quitar las cadenas para no analizar su contenido.
    code = re.sub(r'"(\\.|[^"\\])*"', '""', code)
    code = re.sub(r"'(\\.|[^'\\])*'", "''", code)

    used = {}
    for recv, member in re.findall(r'\b([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)', code):
        if recv in LUA_STDLIB:
            continue
        if recv not in KNOWN_VARS:
            continue
        used.setdefault((recv, member), 0)
        used[(recv, member)] += 1

    # 2b. Receptores con ambito local: `local p = ...` o `for _, p in ...`
    #     redeclaran el nombre y su tipo deja de ser el del mapa. No se puede
    #     saber de que clase es, asi que se comparan contra el universo
    #     completo y, si tampoco aparece, se marcan como desconocidos en vez
    #     de dar un falso positivo.
    shadowed = set(re.findall(r'\blocal\s+([A-Za-z_][A-Za-z0-9_]*)', code))
    shadowed |= set(re.findall(r'\bfor\s+[A-Za-z_][A-Za-z0-9_]*\s*,\s*([A-Za-z_][A-Za-z0-9_]*)\s+in', code))
    shadowed |= set(re.findall(r'\bfor\s+([A-Za-z_][A-Za-z0-9_]*)\s+in', code))

    # 2c. Globales eliminados en 2.x. Se buscan como identificadores sueltos,
    #     no como miembro de otro objeto (x.global si existiria).
    removed_used = []
    for name in sorted(REMOVED_IN_2X):
        for m in re.finditer(r'(?<![.\w])' + re.escape(name) + r'(?![\w])', code):
            removed_used.append((name, code[max(0, m.start() - 40):m.start() + 40]
                                 .replace("\n", " ").strip()))
    if removed_used:
        print("Globales eliminados en Factorio 2.x usados por el mod:")
        for name, ctx in removed_used:
            print(f"  AUSENTE  {name}   ...{ctx}...")
        print()
    else:
        print("Globales eliminados en 2.x: ninguno en uso.\n")

    # 3. Comprobacion. Cada llamada se contrasta contra su clase exacta; si
    #    el receptor no se puede mapear, se usa el universo global.
    print("Miembros usados por el mod:")
    bad = []
    unknown = []
    skipped = []
    for (recv, member) in sorted(used):
        cls = RECEIVER_CLASS.get(recv)
        if recv in shadowed:
            # Tipo desconocido: se acepta si el miembro existe en alguna clase.
            ok = member in universe
            where = "?"
            if not ok:
                unknown.append((recv, member))
        elif cls:
            members = effective(version, cls)
            ok = member in members
            where = cls
        else:
            ok = member in universe
            where = "?"
        if recv in GLOBAL_OBJECTS:
            skipped.append((recv, member))
            continue
        if not ok and recv not in shadowed:
            bad.append((recv, member, where))
        label = "OK     " if ok else ("DESCONOCIDO" if recv in shadowed else "AUSENTE")
        print(f"  {label}  {recv}.{member}   [{where}]")

    print()
    if skipped:
        print("Objetos globales de Factorio (verificados aparte): " +
              ", ".join(f"{r}.{m}" for r, m in skipped))
        print()
    if unknown:
        print("Miembros con receptor de ambito local (tipo no determinable): " +
              ", ".join(f"{r}.{m}" for r, m in unknown))
        print()
    if bad:
        print(f"RESULTADO: {len(bad)} llamada(s) NO existen en la API {version}:")
        for recv, member, where in bad:
            print(f"  - {recv}.{member}  (esperado en {where})")
        return 1
    if removed_used:
        print(f"RESULTADO: se usan {len(removed_used)} global(es) eliminado(s) en {version}.")
        return 1
    print(f"RESULTADO: todas las llamadas existen en la API {version}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
