# ADR-022 — El GUI del juego como textura capturada, y un puntero para interactuar

> **Estado**: diseño cerrado (decidido el 2026-10-25). **Implementación a
> medias, con la parte 3 (capturar el GUI) bloqueada por un dato medido** (ver
> la sección 9).
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


---

## 9. Lo que dice la medicion (2026-10-26)

Antes de escribir codigo se midio en el motor real (el binario `factorio` ARM64
y el cliente de verdad, en el contenedor), porque el diseno de la seccion 3
suponia cosas que hay que comprobar:

### 9.1 El GUI NO va en el framebuffer del mundo

El inyector manda al visor el framebuffer del MUNDO (`FB_MUNDO`).  Ese frame
**no trae el GUI**: capturado a PNG se ve el terreno, las vias y el jugador, y
**ni hotbar, ni minimapa, ni alertas**.  El GUI del motor se dibuja en otro
framebuffer y se compone despues.

Esto significa que la tecnica "GUI = F - W" de la seccion 3 **no se puede
aplicar tal cual**: no hay un "F con el GUI" disponible en el framebuffer del
mundo, y el FBO 0 (la pantalla) con el driver `offscreen` **esta vacio**
(medido: 0 pixeles con color, y 0 de 230400 pixeles distintos del mundo).

### 9.2 El GUI tampoco pasa por `DrawQueue::draw`

El hook de `DrawQueue::draw` recibe la capa (`RenderLayer::Enum`) y por ahi se
ve el mundo.  Las capas medidas en un frame:

```
92:166 draws=[-2525,-2541..3057,2523]   <- el mundo
66:40  draws=[-2945,-1918..113,915]     <- el mundo (otra capa)
124:1  draws=[1152,-1920..1152,-1920]   <- una sola vez
118:1  draws=[1152,-1920..1152,-1920]   <- una sola vez
10:1   draws=[2433,-2030..2433,-2030]   <- una sola vez
```

Ninguna de las tres ultimas es el GUI (el GUI dibujaria hotbar, minimapa y
alertas, o sea **muchos** draws en coordenadas de pantalla, no uno).

### 9.3 `FramebufferOpenGL::blitTo()` no se llama

El detour se instala bien (`detour instalado sobre
FramebufferOpenGL::blitTo()`) pero **el hook no se ejecuta nunca**: esa via
no es la que compone el GUI en 2.1.22.

### 9.4 Lo que ya esta hecho

- El **HUD propio del visor** (las ventanas holograficas con la tabla de
  controles) **solo se dibuja cuando el motor no manda imagen**.  Con el juego
  en marcha, lo que se ve es el GUI del JUEGO (el del motor), que es lo que
  se ve "como en el PC".  Antes los dos a la vez: el minimapa propio se
  comia media vista y la escena se veia distinta del juego de PC.
- Los hooks de diagnostico en el inyector (`XFACTORVR_DIAG_CAPAS`,
  `XFACTORVR_DIAG_PANTALLA`, `XFACTORVR_CAPTURA_GUI`), que son los que han
  destapado los puntos 9.1 a 9.3.

### 9.5 El paso que falta

Localizar el framebuffer del GUI.  Los candidatos que quedan:

1. `RenderUtil::takeScreenshots()`: es donde el motor lee el frame final
   (con el GUI) para escribir la captura.  Enganchandolo se ve el
   framebuffer que usa.
2. `GraphicsInterfaceOpenGL::draw()`: el dibujado final a la pantalla; el
   FBO activo ahi dira si el GUI se compone ahi o en otro.
3. Los nombres del GUI en 2.1.22 no son los de 2.0: no existe
   `GuiRenderTarget` ni `ActiveRenderTarget` (el motor tiene `FrameBuffer`,
   `EngineFramebuffers`, `SpriteRenderTarget`...).  Hay que buscar el
   equivalente actual.

Con ese framebuffer localizado, la idea del ADR-022 se aplica tal cual: el
inyector lo lee, lo manda por XFV1 con un tipo nuevo y el visor lo pinta con
alfa encima del mundo 3D (con las mismas coordenadas que la textura del
panel, de modo que el hit-test del puntero ya sirve).
