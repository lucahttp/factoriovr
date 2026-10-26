# ADR-027 — FORK: el juego plano en una pantalla flotante, el mando de joystick y ratón

> **Estado**: decisión del usuario (2026-10-26). Implementación en curso.
> Revisa: `ADR-026` (la mesa es el juego), `ADR-022` (GUI como textura).

---

## 1. La decisión

Se abandona la "mesa 3D" como vista principal. El objetivo pasa a ser:

> **Poder jugar el juego plano, como si fuera una pantalla flotante**, con los
> controles del mando actuando de joystick y de ratón.

Es el patrón de los ports de VR que ya se usan (un monitor en el aire delante
de ti), y es **mucho más simple** que reconstruir el mundo en 3D en el visor:

* el juego se ve **tal cual lo dibuja el motor** (pixel por pixel);
* sus menús son los suyos (hotbar, inventario, recetas);
* el ratón del juego se mueve con el rayo de la mano;
* el joystick se mueve con el stick.

## 2. Por qué cambia

El problema de fondo era que **reconstruir el mundo en el visor dejaba fuera
el GUI del motor** (ADR-022 §9: el motor compone el HUD en otro framebuffer),
y el HUD propio acababa tapando la escena para sustituirlo.

Con una pantalla flotante:

* el GUI del motor se puede **superponer en la misma pantalla** (mismas
  coordenadas de píxel ⇒ el ratón del motor y el GUI coinciden solos);
* el mundo en 3D del mod de GDani31 deja de ser imprescindible para
  *jugar* (sigue siendo el plan de la mesa, en el otro modo).

## 3. Diseño

### 3.1 La pantalla

* Un quad **vertical** (tipo cine) delante del usuario, con el frame del
  motor (640×360, 16:9).
* Tamaño y distancia configurables (la "mesa" actual pasa a ser el soporte
  de la pantalla).
* Se dibuja con el mismo shader del panel de hoy (`draw_game_panel`), que
  ya sabe colocar el píxel del motor bajo el rayo de la mano.

### 3.2 Los controles

| Mando | Acción |
|---|---|
| Rayo de la mano (puntero) | mueve el ratón del motor (clic XFCE ya implementado) |
| Gatillo | clic izquierdo / soltar |
| Stick izquierdo | **joystick del juego** (WASD): el inyector traduce el stick a eventos de teclado |
| Grip | clic derecho / menú |
| B | abrir/cerrar los paneles flotantes |

El puente de entrada (`injector-linux/src/entrada.rs`) ya sabe inyectar
teclas y movimiento del ratón en el motor: sólo falta traducir el stick.

### 3.3 Los menús

* **Los del juego**: el GUI del motor, capturado del framebuffer que lo compone
  (ADR-022 §9.5) y superpuesto en la pantalla, en las mismas coordenadas.
* **Los nuestros** (construir rápido, vista remota): pasan a ser paneles
  flotantes aparte, como las ventanas de Generals, y dejan de estar delante de
  la cara.

### 3.4 Qué se queda y qué se va

* **Se queda**: el motor, el inyector, el panel (con el cambio a pantalla),
  el puntero, el puente de entrada, el canal de geometría.
* **Pasa a opcional**: el diorama 3D reconstruido por el visor (queda como
  vista remota / modo observador).
* **Se aparta**: el HUD propio en el espacio de la cabeza (se va a panel
  flotante).

## 4. El bloqueo que hay que resolver antes

El motor **entra en juego y renderiza** (medido: `to(InGame)`,
`PlayerJoinGame`, 6.000 frames), pero el inyector **sólo consigue mandar 3
frames**: el resto los descarta por "vacíos" (`vacias` en el log). El motor
dibuja en negro con el driver Turnip (en el contenedor, con lavapipe, el
frame sale con la imagen del terreno).

**Diagnóstico pendiente**: volcar un frame a PNG en el visor
(`XFACTORVR_3D_CAPTURA=/tmp/f.png`) para confirmar que sale negro, y
comparar con el cliente usando lavapipe.

## 5. Orden de trabajo

1. Confirmar (y arreglar) por qué el motor dibuja en negro con Turnip.
2. Que el visor pinte el frame del motor en una pantalla vertical flotante.
3. Stick izquierdo → joystick del juego.
4. GUI del motor superpuesto en la pantalla (ADR-022 §9.5).
5. Paneles flotantes nuestros (mover/escalar) y el HUD propio fuera de la cara.
