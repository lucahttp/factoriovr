#!/usr/bin/env python3
"""
preparar_campana_mp.py — Deja el nivel de campaña listo para multijugador.

Por qué hace falta
-----------------
El nivel de campaña es una copia de `base/campaigns/tutorial/level-01` de
Factorio 2.1.22, pensado para singleplayer. En un servidor headless el
escenario arranca SIN jugadores (el cliente gráfico se conecta después) y
el nivel peta con:

    __level__/control.lua:132: attempt to index field '?' (a nil value)
    __core__/lualib/story.lua:71: in function 'story_update'
    -> "The scenario level caused a non-recoverable error."

Ademas, el nivel usa `game.show_message_dialog`, que en multijugador falla
siempre con "Map doesn't contain 1 player, this function can't be used"
(comprobado contra el motor real, ver docs/ADR-005).

Los dos parches
--------------
1. `init` de la rama 'setup': si todavia no hay jugador, rearma los
   marcadores de "init ya llamado" y devuelve `true` (el módulo `story`
   aborta ese evento). El siguiente tick reintenta, asi que la historia
   arranca sola en cuanto el cliente gráfico entra y el servidor crea al
   jugador.

2. `game.show_message_dialog(...)` -> `story_show_message_dialog(...)`:
   el helper del módulo `story` (ya cargado por el nivel con
   `require("story")`) es el que sabe qué hacer en multijugador (imprime el
   texto al jugador en vez de abrir el diálogo de un jugador concreto).

Es idempotente: si el control.lua ya está parcheado no lo vuelve a tocar,
y siempre deja una copia intacta en `<escenario>/control.lua.orig`.

Uso:
    python3 scripts/preparar_campana_mp.py /ruta/al/escenario
    python3 scripts/preparar_campana_mp.py /ruta/al/escenario --copiar-como nombre
"""
import argparse
import os
import shutil
import sys

# --- parche 1: esperar al jugador -------------------------------------------
ORIGINAL_INIT = """      name = 'setup',
      init = function()
        game.players[1].disable_recipe_groups()
"""

PARCHE_INIT = """      name = 'setup',
      init = function(event, st)
        -- [XFactorVR-MP] El nivel original asume singleplayer: cuando el
        -- escenario arranca ya hay un jugador. En un servidor headless el
        -- cliente grafico entra DESPUES, asi que aqui se espera a que
        -- exista. Se rearma el marcador de "init ya llamado" para que el
        -- modulo story vuelva a intentarlo en el siguiente tick.
        if not game.players[1] then
          st.init_called_last_on_story_index = 0
          st.init_called_last_on_position = 0
          return true
        end
        game.players[1].disable_recipe_groups()
"""

# --- parche 2: dialogos de multijugador -------------------------------------
ORIGINAL_DIALOG = "game.show_message_dialog("
PARCHE_DIALOG = "story_show_message_dialog("


def parche_init(texto: str):
    """Espera al jugador. Devuelve (texto, aplicado)."""
    if "-- [XFactorVR-MP]" in texto:
        return texto, False
    if ORIGINAL_INIT not in texto:
        print("  AVISO: no encuentro el patron de 'setup' (¿nivel nuevo?)")
        return texto, False
    return texto.replace(ORIGINAL_INIT, PARCHE_INIT, 1), True


def parche_dialog(texto: str):
    """show_message_dialog -> story_show_message_dialog. (texto, aplicado)"""
    if PARCHE_DIALOG in texto:
        return texto, False
    n = texto.count(ORIGINAL_DIALOG)
    if n == 0:
        return texto, False
    return texto.replace(ORIGINAL_DIALOG, PARCHE_DIALOG), True


def parchear(control_lua: str) -> bool:
    """Parchea el control.lua in situ. Devuelve True si lo ha cambiado."""
    with open(control_lua, encoding="utf-8") as fh:
        original = fh.read()

    texto, c1 = parche_init(original)
    texto, c2 = parche_dialog(texto)

    if not (c1 or c2):
        print(f"  ya parcheado: {control_lua}")
        return False

    # Copia intacta por si hay que deshacer el parche.
    orig = control_lua + ".orig"
    if not os.path.exists(orig):
        shutil.copy2(control_lua, orig)
        print(f"  copia intacta: {orig}")

    with open(control_lua, "w", encoding="utf-8") as fh:
        fh.write(texto)
    print(f"  parcheado: {control_lua}")
    if c1:
        print("    · espera al jugador antes de arrancar la historia")
    if c2:
        print("    · show_message_dialog -> story_show_message_dialog (multijugador)")
    return True


def copiar_como(origen: str, destino: str) -> None:
    """Copia el escenario a otro nombre (para no tocar el original)."""
    if os.path.exists(destino):
        print(f"  ya existe, no se copia: {destino}")
        return
    os.makedirs(destino, exist_ok=True)
    for nombre in ("control.lua", "blueprint.zip", "description.json"):
        src = os.path.join(origen, nombre)
        if os.path.exists(src):
            shutil.copy2(src, os.path.join(destino, nombre))
    locale = os.path.join(origen, "locale")
    if os.path.isdir(locale):
        shutil.copytree(locale, os.path.join(destino, "locale"), dirs_exist_ok=True)
    print(f"  escenario copiado: {destino}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("escenario", help="directorio del escenario (el que tiene control.lua)")
    ap.add_argument("--copiar-como", default=None,
                    help="nombre de un escenario nuevo al que copiar (en el mismo padre)")
    args = ap.parse_args()

    escenario = os.path.abspath(args.escenario)
    control = os.path.join(escenario, "control.lua")
    if not os.path.isfile(control):
        print(f"ERROR: no existe {control}")
        return 2

    print(f"Escenario: {escenario}")
    if args.copiar_como:
        padre = os.path.dirname(escenario)
        destino = os.path.join(padre, args.copiar_como)
        copiar_como(escenario, destino)
        escenario = destino
        control = os.path.join(escenario, "control.lua")

    parchear(control)
    return 0


if __name__ == "__main__":
    sys.exit(main())
