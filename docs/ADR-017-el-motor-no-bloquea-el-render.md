# ADR-017 — El motor no puede bloquear el render: el sondeo va en un hilo aparte

> **Estado**: implementado y **medido en la Pico 4 Ultra** (2026-10-24).
> Revisa: `ADR-013` (el motor dibuja), `ADR-015` (el cliente en la Pico).

---

## 1. El síntoma

En el dispositivo, con el motor headless + el visor MR:

```text
FPS=15/90  CPU=62.91ms  GPU=2.71ms      ← antes
FPS=13/90  CPU=61.06ms  GPU=2.15ms      ← idem
```

La GPU hacía nada (2-3 ms) y la CPU se comía 62 ms por frame. La tabla se
movía a trompicones y elpassthrough consumía el resto.

## 2. La causa

`xfactorvr_update_simulation()` se llama **una vez por frame** y dentro hacía
**la E/S con el motor**: la consulta del snapshot por RCON.

Medido contra el motor real (Pico, RCON redirigido al Mac):

| Operación | Tiempo |
|---|---|
| Auth RCON | 4 ms |
| `rcon.print('listo')` (calentamiento) | 6 ms |
| `remote.call('xfactorvr','get_world_snapshot', 32, 12, -3)` | **61 ms** (26 729 bytes de JSON) |

Con `RCON_POLL_INTERVAL = 0.12 s` (8 sondeos/s), el hilo de render se
bloqueaba **61 ms cada 120 ms** ⇒ 13-15 FPS. Cuadra exacto con la medición.

O sea: **no era que el motor fuera lento, era que el render esperaba al motor.**

## 3. El arreglo

`poll_rcon_sync()` **ya no hace E/S**:

1. El render anota el centro de la vista (dos `AtomicU32` con los bits del
   `f32`) y **recoge lo que haya en el buzón**; si no hay nada nuevo, no
   devuelve nada (así el render **no reparsea 26 KB de JSON 90 veces por
   segundo**).
2. Un **hilo propio** (`xfv-sondeo`) consulta al motor con su **propia sesión
   RCON** (cliente aparte del de órdenes: si compartieran socket, una consulta
   lenta congelaría al otro).
3. La sesión se mantiene viva; si el motor no responde, se reinicia **una**
   vez (como antes) y se entrega el resultado al buzón.

Con el RCON en un hilo aparte, el render nunca espera.

## 4. El resultado (medido en la Pico 4 Ultra)

```text
FPS=90/90  CPU=2.25ms  GPU=4.06ms
FPS=90/90  CPU=2.34ms  GPU=3.87ms
FPS=90/90  CPU=2.15ms  GPU=3.53ms
FPS=90/90  CPU=2.22ms  GPU=3.71ms
```

**De 13 FPS a 90 FPS (el ritmo nativo del visor)**, con la CPU del render en
2,2 ms y la GPU en 3,7 ms. Y el puente vivo:

```text
libxfactorvr: Instantanea aplicada: 35 entidades del juego real (0 sueltos)
```

El visor dibuja la fábrica real del motor, en MR, a 90 Hz.

## 5. Nota sobre "predicción" y "RCON"

Esto **no** es predicción: no se inventa estado. Es **amortización**: el motor
se consulta 8 veces por segundo y el render usa el último estado recibido. El
estado que se ve es el real del motor (el mismo JSON que aplica el mod), y
entre sondeos el render va a 90 Hz con lo último que llegó.

La alternativa de fondo (que el inyector empuje la escena al visor, sin RCON)
sigue siendo mejor — el inyector ya la tiene cuando el motor dibuja (V1) —
pero no es necesaria para que la experiencia sea jugable.

## 6. Lo que queda abierto

- El **atlas de suelo** no está (`no hay atlas de suelo (paginas=0)`): el suelo
  de la mesa se dibuja sin textura.
- El HUD sigue en la "middle distance" (ver `ADR-016` §5.2).
- El cliente gráfico del motor en la Pico (V1) sigue sin contexto EGL
  (`ADR-015` §4).
