# ADR-026 — Diseño: la mesa ES el juego (patrón Generals-Zero-Hour-XR + factorio-3d-mod)

> **Estado**: decisión tomada el 2026-10-26, a partir de lo medido.
> Revisa: `ADR-022` (GUI como textura), `ADR-021` (canal de geometría),
> `ADR-025` (el motor lo carga el linker).

---

## 1. Lo que dicen las dos referencias

### 1.1 Generals-Zero-Hour-XR (el patrón de UI)

Su README (lo importante):

> *"Native OpenXR stereo tabletop gameplay using the original **Zero Hour
> game world, not replacement models**."*
>
> *"…with separate spatial **build, command and settings windows**."*
>
> *"Detached build window, persistent Commands window and a spatial UI panel;
> **move, tilt, scale or recenter the workspace**."*
>
> *"cinematics and full native dialogs appear **upright**."*
>
> Controles: **A** = mostrar/ocultar Commands, **B** = menú del juego.

Es decir, el patrón es:

1. **La mesa es el juego real**, no una reconstrucción.
2. **El juego se ve en 3D de verdad** (el motor dibuja con cámara 3D).
3. **Los menús del juego son los del juego** (diálogos nativos, verticales).
4. **Encima, ventanas flotantes** separadas: construir, órdenes, ajustes. Se
   mueven, inclinan, escalan y recentran.
5. El HUD de VR son *placas* (shortcut plates) al lado del tablero.

### 1.2 GDani31/factorio-3d-mod (el 3D)

* Es un **inyector** (DLL) como el nuestro: engancha el render del motor.
* **El 3D lo hace el MOTOR**, dentro de su propio framebuffer: la cámara se
  rota, inclina y hace zoom (Shift + arrastre), y hay vista en primera
  persona.
* Su frase clave: **"Menus, GUIs and mouse picking work as usual."**

O sea: el 3D **no se reconstruye en el visor**; el motor lo dibuja y sus
menús siguen siendo los suyos, funcionando.

---

## 2. Lo nuestro, y donde diverge

| | Referencias | Nuestro (hoy) |
|---|---|---|
| Mesa | el juego real, 3D del motor | **reconstruimos el mundo en el visor** (renderer propio, modelos GLB instanciados) |
| 3D | lo dibuja el motor (cámara 3D) | lo dibuja nuestro renderer (la cámara es la del motor, ortográfica) |
| GUI del juego | los menús nativos, verticales | **no llegan**: el motor los compone en otro framebuffer (ADR-022 §9) |
| Ventanas flotantes | build / commands / settings, movibles | el HUD propio (construir/inventario/fabricar/vista) **en el espacio de la cabeza**, no movible |
| Mundo real | sí | **sí**: 35 entidades reales medidas (21 ensambladoras, 4 cofres, 2 hornos, taladro, cinta, torretas) |

**El punto ciego grande**: al reconstruir el mundo en el visor, el GUI del
motor se queda fuera, y con él toda la UI nativa (hotbar, inventario,
recetas, ordenes). Por eso el HUD propio grows hasta tapar la escena: está
sustituyendo a la UI del juego.

**⇒ La intuición del usuario es la correcta**: para "ver el mapa real en 3D
con los menús de Factorio", el camino no es reconstruir más, es **dejar que
el motor dibuje el 3D** (el mod lo hace, y nuestro inyector ya lo hace
también) y **llevar su GUI al visor**.

---

## 3. Lo que ya tenemos, en el camino correcto

* El inyector **ya sustituye los sprites por modelos GLB dentro del
  framebuffer del motor** (módulo 3D) ⇒ el frame que llega al visor ya es
  el mundo en 3D, como el del mod.
* El motor **ya entra en juego** en el visor con jugador real (medido:
  `to(InGame)` + `PlayerJoinGame mode(connect)`), y el mod empuja el estado
  real del mundo (35 entidades + la posición del jugador).
* El puntero y el clic XFCE (el ratón del motor) ya existen ⇒ el
  *mouse picking* del mod ya está.
* El HUD propio (construir / inventario / fabricar / vista) es el equivalente
  de las ventanas de Generals, pero **en el espacio de la cabeza**.

## 4. Lo que falta para poder jugar de verdad

Ordenado por lo que desbloquea jugar:

1. **Que la imagen del motor se vea en la mesa** (el panel). Ahora mismo el
   panel sale blanco: el motor manda frames pero la textura no se pinta.
   *Es el bloqueante número uno.*
2. **El GUI del motor como textura** (ADR-022 §9): es lo que da "los menús de
   Factorio". El motor lo compone en un framebuffer que todavía no hemos
   localizado (candidatos medidos en ADR-022 §9.5).
3. **El passthrough** (el modo de mezcla ya es conmutable por
   `debug.XFACTORVR_BLEND`; falta que se vea la habitación).
4. **Las ventanas flotantes que se mueven**, como en Generals (nuestro HUD
   propio está clavado en el espacio de la cabeza).
5. **La cámara 3D del motor** (rotar/inclinar/zoom como el mod), para que la
   mesa se vea de verdad en 3D y no con la proyección ortográfica del juego.

## 5. Decisiones que se toman aquí

1. **El panel del motor es la superficie principal.** Nuestro renderer 3D
   propio pasa a ser la **vista remota** (segunda cámara, ADR-022 §6), no la
   vista principal. Motivo: es lo que hacen las dos referencias y es lo que
   hace falta para tener los menús del juego.
2. **El HUD propio deja de ser el sustituto del GUI del motor.** Se queda
   como ventanas auxiliares (construir rápido, vista remota) y se les añade
   movimiento, inclinación y escala como en Generals.
3. **El 3D lo hace el motor** (el inyector), no el visor. SeLevera el
   renderer propio del camino principal; no se tira (sirve para la vista
   remota y para el modo observador).
4. **El GUI nativo del motor es el objetivo de UI**, con el HUD propio como
   complemento.
