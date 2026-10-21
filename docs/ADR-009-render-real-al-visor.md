# ADR-009 — El render real del juego llega al visor VR

## Contexto

Hasta ahora el visor (el APK de la Pico 4 Ultra) **no pintaba el juego**: pintaba
una reconstrucción del mundo con los sprites y el terreno extraídos del motor
(atlas de `build/factorio_assets/` + malla del suelo que calcula Rust). Es una
aproximación fiel en la forma, pero no es el render del juego.

El objetivo del proyecto es al estilo GZH-XR: **el juego corre de verdad y su
pipeline de render alimenta la vista estéreo**. El inyector ya sabe dibujar el
cubo 3D en el framebuffer real del juego (ADR-006), así que la pieza que faltaba
era el **transport**: que esa imagen llegue al visor.

Restricciones del dispositivo que condicionan el diseño:

| Restricción | Consecuencia |
|---|---|
| El APK corre en `untrusted_app` | no puede leer `/data/local/tmp` ni `/sdcard` freely ⇒ nada de ficheros |
| El motor (glibc, en Termux/proot) y el APK (bionic) son procesos distintos | cada uno tiene su propio contexto y su propio espacio de nombres |
| La Pico no tiene GPU para el juego (lavapipe) | el juego va a pocos FPS; el stream tiene que poder ir a 10-15 FPS |
| El juego se puede reiniciar (o el visor) | el transporte tiene que tolerar reconexiones |

## Decisión

**El inyector lee el framebuffer del mundo y lo manda por TCP al visor
(protocolo "XFV1"). El visor lo pinta como un panel en MR con un desplazamiento
de UV por ojo.**

### El emisor (inyector)

`injector-linux/src/stream.rs`:

- El hook de `DrawCommandBatch::flushSpritesBatch` (y sus hermanos) ya se
  dispara **justo después de que el motor ha pintado el mundo** en el
  framebuffer del mundo. Ahí se lee el framebuffer con `glReadPixels` y se
  manda.
- El hilo de render **nunca** escribe en el socket: el frame se mete en un
  canal con capacidad 2 y lo escribe un hilo aparte. Si el visor va lento se
  tira el frame viejo (la imagen tiene que ser la última, no la más antigua).
- La imagen se reduce al tamaño objetivo (640×360 por defecto) con un filtro de
  caja, en el hilo del juego: mandar 1280×720 por el socket es mucho.
- El límite de FPS se consulta **antes** de leer el framebuffer (leer es lo
  caro: 3,6 MB por frame).
- **Los frames en negro no se mandan.** El motor dibuja en negro al cargar y
  en los menús; en el visor eso son parpadeos. Si menos del 1 % de los píxeles
  tienen color, el frame se descarta y el visor se queda con la última imagen
  buena.
- **El cubo se vuelve a pintar justo antes de leer.** El motor redibuja el
  terreno por encima de lo que hemos pintado, así que el cubo 3D puede haber
  desaparecido del framebuffer entre una pasada y otra: sin esto, solo 1 de
  cada 30 frames que se mandaban llevaba el 3D dentro.

Variables de entorno:

| Variable | Por defecto | Qué hace |
|---|---|---|
| `XFACTORVR_STREAM` | (vacío = no se transmite) | `host:puerto` del visor |
| `XFACTORVR_STREAM_ANCHO` | 640 | ancho del frame |
| `XFACTORVR_STREAM_ALTO` | 360 | alto del frame |
| `XFACTORVR_STREAM_FPS` | 15 | frames por segundo objetivo |
| `XFACTORVR_STREAM_DEBUG_PNG` | (vacío) | escribe un PNG de lo que se manda (diagnóstico) |

### El protocolo XFV1

TCP, little-endian. El emisor es el **cliente** y el visor el **servidor**
(al revés que en un juego en red): así el visor puede arrancar y parar sin que
el juego se entere, y el juego reconecta solo si el visor se reinicia.

```
cabecera (16 bytes)
   0..4    magic   u32 = 0x31564658  ("XFV1")
   4..8    numero de frame  u32
   8..10   ancho   u16
   10..12  alto    u16
   12      flags   u8   (bit 0: invertida verticalmente)
   13      formato u8   (0 = RGBA8888)
   14..16  reservado u16
carga: ancho * alto * 4 bytes RGBA
```

Las filas van en orden de GL (la fila 0 es la de abajo): al subirlo como
textura se sube tal cual y el shader invierte la V.

### El receptor (APK)

`android/src/openxr_engine.cpp`:

- Un hilo receptor abre `127.0.0.1:27020` y guarda el último frame. **No toca
  el GL**: el hilo de OpenXR es el único que sube la textura.
- El panel es un quad vertical (1 m × 16:9) al fondo de la mesa, con la
  textura del juego.
- **La V del panel va invertida**: el motor dibuja el mundo con la Y
  invertida, así que la fila 0 del framebuffer es la de **arriba** de la
  imagen. Se comprobó capturando la pantalla real del juego en el contenedor
  (`import -window root` sobre el Xvfb) y comparándola con el frame que viaja
  por el socket: las dos imágenes tienen la misma orientación (la maquinaria
  grande arriba a la izquierda, el terreno abajo, el cubo 3D a la derecha). Al
  subir el buffer tal cual a la textura, la `t=0` queda en la fila 0, así que
  la `t=0` tiene que pintar la parte de **arriba** del panel.
- **El paralaje**: cada ojo desplaza la UV un 1,8 % en horizontal
  (`u_uv_offset`). Es el efecto "TV 3D": la imagen se ve como una ventana al
  mundo y el cubo 3D que pinta el inyector gana volumen. El shader ya tenía
  `u_uv_offset` de antes (se usaba para el terreno), no ha hecho falta tocar
  los shaders.

## Alternativas descartadas

- **Memoria compartida (`/dev/shm` o un `memfd`)** — más rápido, pero el juego
  (en Termux, con su propio espacio de nombres y sus permisos) no puede
 mapear un segmento del APK, y al revés tampoco.
- **Ficheros en `/data/local/tmp` o `/sdcard`** — el APK corre en
  `untrusted_app` y no puede leerlos; y en Android 11+ el acceso al
  almacenamiento compartido por ruta arbitraria es restringido.
- **UDP con datagramas** — hay que fragmentar a mano (un frame de 640×360 son
  900 KB, muy por encima del MTU) y perder un fragmento rompe el frame. TCP en
  loopback va a 9 MB/s sin problema y ya trae el troceado.
- **Que el visor renderice el juego en vez de recibir la imagen** — es
  reinventar el motor; el inyector ya está dentro del proceso y tiene el
  framebuffer a mano.

## Limitaciones conocidas

- **No es estéreo de verdad**: se manda una sola vista y el paralaje se
  simula desplazando la UV. El juego no se renderiza dos veces (una por ojo),
  así que el efecto 3D es el de una "TV 3D", no el de un par de vistas reales.
  Para/views reales habría que volver a renderizar el mundo con la cámara
  desplazada, que es trabajo del motor, no del inyector.
- **La vista es la de la cámara del juego**, no la de la cabeza: el panel no
  sigue la mirada.
- **La frecuencia la manda el juego**: con lavapipe en la Pico el juego va a
  pocos FPS, así que el panel va a parpadear menos de lo que gustaría. Por eso
  el buffer del canal es de capacidad 2 y se descartan los frames viejos.
- El panel se pinta **encima** de la mesa (con el mismo shader y blending), así
  que si el juego está en una zona con GUI (menús) esa GUI no sale: lo que se
  manda es el framebuffer del mundo, no el de la ventana.

## Cómo se verifica

`scripts/test_stream_render.sh` (contenedor podman, cliente gráfico ARM64 real):

1. Compila el inyector para `aarch64-unknown-linux-gnu`.
2. Arranca el cliente gráfico con el inyector y `XFACTORVR_STREAM=127.0.0.1:27020`.
3. Crea una máquina con la consola del juego (para que haya entidad que
   dibujar en 3D).
4. Un receptor (`scripts/receptor_stream.py`) cuenta los frames que llegan y
   busca **pixeles del cubo 3D** dentro de la imagen.

Verde = llegaron frames **con el cubo 3D dentro**, o sea: la imagen que compone
el motor (con el 3D) viaja al visor.

La evidencia visual es el PNG de diagnóstico (`build/stream_debug_small.png`):
es literalmente el frame que viaja por el socket, con el terreno real del
juego, las máquinas y el cubo magenta del módulo 3D.
