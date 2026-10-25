# ADR-022 — El GUI del juego como textura capturada, y un puntero para interactuar

> **Estado**: diseño cerrado (decidido el 2026-10-25). Implementación pendiente.
> Revisa: `ADR-021` (el canal de geometría), `ADR-020` (sin RCON).

---

## 1. La decisión

El objetivo es lo que enseñan las capturas de `GDani31/factorio-3d-mod`: **el
mundo en 3D sobre la mesa y los menús del juego (hotbar, minimapa, alertas)
encima**.

Se decide **no** seguir con un HUD propio inventado para el GUI del juego, y
sí con:

1. **El GUI del juego como textura capturada.**  El motor lo dibuja; el
   inyector lo captura y el visor lo pinta tumbado en la mesa.
2. **Un puntero** para interactuar con esa textura (y con el mundo 3D).

## 2. Por qué la textura y no el HUD propio

El mod de referencia **no parchea el motor**: es un inyector (un DLL que
hookea el motor, igual que el nuestro) y su README dice *"Menus, GUIs and
mouse picking work as usual"*. Es decir: **el motor dibuja el mundo en 3D y el
GUI encima, sin tocar nada**.

Nuestro inyector **ya hace la primera parte**: el módulo 3D sustituye el
sprite 2D de cada entidad por su modelo GLB dentro del framebuffer del motor
(`dibujar_modelos`), con lo que el frame que sale del motor **ya es el mundo
en 3D con el GUI encima** ✓.

Lo que falta es llevar **ese GUI** al visor, donde el mundo se reconstruye en
3D estéreo. De ahí la textura.

## 3. Cómo se captura el GUI

El GUI se dibuja **encima** del mundo, en el mismo framebuffer y **en otra
capa** (`RenderLayer`). El inyector ya ve esa capa: el hook de
`DrawQueue::draw` recibe el `layer` de cada draw call ✓.

**Técnica: la diferencia de dos capturas.**

```text
   frame del motor
   ├─ se dibuja el mundo (capa 0..N)
   ├─  >>> aqui el inyector lee el framebuffer: W  (el mundo SIN el GUI)
   ├─ se dibuja el GUI (capa GUI)
   └─  >>> aqui el inyector lee el framebuffer: F  (el mundo CON el GUI)

   GUI = F - W   (donde difieren, con alfa proporcional)
```

- El inyector **no toca el render**: solo lee dos veces el framebuffer (ya lo
  lee una al final de cada frame ✓).
- El **visor** hace la resta (0,9 MB por frame: trivial) y saca una textura
  **RGBA con alfa**: el color del GUI y el alfa de cuánto se despegó de W.

El resultado se manda por el mismo protocolo XFV1, con un **tipo de mensaje
nuevo** (flag bit 2): la carga son las dos imágenes (W y F).

## 4. Cómo se pinta y cómo se interactúa

El GUI capturado tiene **las mismas coordenadas de pantalla** que el juego
(640×360), así que:

- El visor lo pinta en **el mismo quad** donde ya va el panel del juego (que
  pasa a contener el GUI con alfa, no el frame entero).
- El **hit-test ya existente** (`s_game_panel_mat` + `s_game_panel_w/h`) sirve
  tal cual: el rayo de la mano → el píxel de la textura → **ese píxel es
  exactamente la coordenada de pantalla del motor** ⇒ el ratón del motor va
  donde apunta el puntero ✓✓.

**El puntero**: un pequeño disco (o una flecha) en el punto donde el rayo
toca la textura, dibujado con el mismo shader del panel. Se pinta en el
punto de impacto del rayo con el widget ✓ (el mismo punto que hoy se usa
para el clic).

## 5. El mundo 3D y el puntero

El mundo en 3D se dibuja con el renderizador que ya existe (terreno con su
atlas, sprites, modelos GLB instanciados, estéreo por ojo). El puntero
sirve para **las dos** cosas:

- sobre el **GUI**: píxel de pantalla → ratón del motor (como arriba);
- sobre el **mundo 3D**: el punto del mundo → la entidad de debajo → la
  acción (colocar, minar) con las coordenadas de tile ✓ (el `view_center` y
  el `model_mesa` ya dan esa conversión).

## 6. El "remote view"

El mod tiene una vista general desde arriba. Con el mundo en 3D **sale
gratis**: es una **segunda cámara** en el visor (misma escena, otra matriz).
Se puede poner en un atril o en un botón del HUD.

## 7. Orden de trabajo

1. **El inyector**: leer el framebuffer en el cambio de capa (W) y al final
   (F) y mandar el mensaje nuevo. Banco: `scripts/probe_xfv1.py` (que ya
   existe) lo verifica ✎ → ampliarlo para el tipo nuevo.
2. **El visor**: recibir W+F, restar, subir la textura con alfa.
3. **El quad**: pintar el GUI con alfa en el sitio del panel actual.
4. **El puntero**: el disco en el punto de impacto + el hit-test (que ya
   está) → el ratón del motor.
5. **El remote view**: la segunda cámara.

## 8. Lo que NO cambia

- El **estado** sigue viniendo del fichero que empuja el mod (ADR-020).
- Las **acciones** siguen yendo por la consola del motor (ADR-020).
- El **render 3D** del visor no se toca: solo se le añade una textura.
- El motor **no se parchea**: todo esto vive en el inyector y en el visor.
