# ADR-016 — ¿Tiene sentido lo que estamos haciendo? Sintonización con el exterior

> **Estado**: documento de análisis (2026-10-24). No cambia la arquitectura;
> **cambia dos decisiones de camino** (ver §5).
> Revisa: `ADR-013` (el motor dibuja), `ADR-015` (el cliente en la Pico),
> `PLAN-PICO-4-ULTRA.md` (F0).

---

## 1. Veredicto corto

**Sí, la arquitectura tiene sentido, y la investigación externa la refuerza en
lugar de desmontarla.** Pero también señala **dos cosas que estábamos
haciendo por el lado equivocado**:

1. El bloqueo de GPU que medimos en la Pico (el kernel rechazando al usuario
   `shell`) **no es un muro**: es un problema de *uid*. Las vías que funcionan
   sin root pasan por **Termux** (que sí puede pedir acceso a la GPU) o por el
   **wrapper de Vulkan de Android** (que expone el driver bionic de Android como
   un ICD de Vulkan para programas glibc).
2. La UX que hemos construido (rayo, "gana el más cercano", ventanas
   espaciales) **coincide con las recomendaciones oficiales** de Meta, Microsoft
   y la literatura de AR para juegos de estrategia. Hay **dos ajustes concretos**
   (distancia del HUD y tamaño mínimo de los objetivos).

Y una conclusión de fondo que cambia el encuadre del proyecto:

> **El enfoque de GZH-XR (portar el motor a Android) es imposible con
> Factorio**, porque su motor es de código cerrado. Nuestro enfoque no es un
> apaño: es **el único posible**. Por eso tiene valor.

---

## 2. Precedentes: qué se ha hecho de verdad

| Proyecto | Qué es | Qué implica para nosotros |
|---|---|---|
| **Generals Zero Hour XR** (Cesarus85) | "Port **nativo** de *Generals: Zero Hour* a **Meta Quest 3**: **OpenXR estéreo de mesa**, con el mundo original del juego, no modelos de sustitución" | Su base es un **port del motor a Android** (`tarek369/GeneralsZH-Android`): se compila el motor para arm64-v8a con el NDK y **DXVK** traduce Direct3D→Vulkan. **Factorio no tiene código fuente**, así que esa vía no existe para nosotros |
| **GeneralsVR** (Gonzorro) | El juego original corriendo en VR **por Quest Link** (PC transmitido) | Es el "panel dentro del VR" clásico: el mismo enfoque que tenía nuestro visor antes de V1 |
| **Factorio en el Steam Frame** | El build oficial linux-arm64 **que Wube hizo para el Frame**, con Zink y 60 FPS a 720p | Prueba que el motor ARM64 + Zink funciona **en un dispositivo Linux VR**. En el Frame el problema no existe porque es Linux con Vulkan abierto; en la Pico es Android con un HAL de gráficos |
| **r/termux**: "Factorio ARM64 running natively on Android" (813 upvotes) | Termux + Termux:X11 + Debian PRoot + traducción GL→Vulkan | **Precedente directo de nuestro enfoque**: el motor oficial ARM64 corriendo en Android. Da 3,5 FPS, pero con la **vía más débil** (VirGL/ANGLE sobre **Mali**) |
| **GZH-XR (UX)** | Tablero horizontal + ventanas de construcción/comandos separadas, "gana el más cercano", sin mando = mover, un dedo = colocar, tracking perdido = no.orderes | Es de donde sale nuestro enrutado de puntero |

**No hay ningún "Factorio VR"**: nadie ha hecho esto. Tampoco hay ningún
proyecto que use RCON para mover un juego: nuestra idea de que el motor reciba
un **ratón** en vez de órdenes es, hasta donde llega la búsqueda, original.

---

## 3. La GPU en Android sin root: la vía que no probamos

Medido en la Pico: `/dev/kgsl-3d0` es 666, pero el kernel registra
`kgsl: illegal gpu render pid` → el usuario `shell` no puede hacer trabajo de
GPU. La documentación de Termux/Droidspaces explica el matiz:

> **Turnip** (Mesa, driver Vulkan de Adreno): *"near native, **the GPU is
> driven from inside the container**"*, con `MESA_LOADER_DRIVER_OVERRIDE=kgsl`
> y `TU_DEBUG=noconform`. En Android, **Droidspaces** lo consigue con el
> interruptor **"GPU Access"**, que es una app (no `adb shell`) la que pide
> acceso a los nodos de GPU.

Es decir: **el acceso a la GPU es de las apps, no del shell de adb**. Por eso
nuestro spike se quedó corto: probamos desde el sitio equivocado.

Las tres vías reales, todas sin root:

| Vía | Qué es | Estado para nosotros |
|---|---|---|
| **A. Termux + Termux:X11** | El servidor X (bionic) hace el trabajo de GPU; el cliente glibc habla **GLX por el socket** → **no hay problema de dos libc** | **Termux y Termux:X11 ya están instalados en la Pico** ✓. Es la receta de r/termux |
| **B. Android Vulkan Wrapper** (PR #22500 de termux-packages, ya *mergeado*) | Envolve el driver Vulkan **bionic de Android** como un **ICD de Vulkan para programas glibc** (con libhybris) → Zink encima → OpenGL 3.3 para el Factorio glibc | Es *exactamente* la pieza que faltaba. En su PR: *"never tested in proot or termux glibc, only tested in lxc"* → **sin probar en nuestro entorno, pero plausible** |
| **C. Fryzek's KGSL** | Vulkan **y** OpenGL nativos de Adreno en un paquete | "experimental y sin mantener" |

Y una corrección a nuestra propioikat pessimisticismo: el post de r/termux
consigue 3,5 FPS con **VirGL/ANGLE sobre Mali** (la vía más pesada). En Adreno
con Turnip, la documentación dice "near native". **Nuestro "da igual el FPS"
puede ser demasiado pesimista.**

---

## 4. La UX: validada por la literatura, con dos ajustes

De las guías de Meta (Hands UI / indirect interactions), Microsoft (Mixed
Reality: cursores) y la tesis *Guidelines for the Design of Augmented Reality
Strategy Games* (Nilsen):

| Regla | Qué dice | Nosotros |
|---|---|---|
| "Indirect interaction es cómodo de **0,8 m a 3 m**; **evita 0,5–0,8 m** (middle distance)" | El HUD no debe caer en la zona demedia | ⚠️ **Nuestro HUD está justo en la mesa** (plano vertical en el borde) → **subirlo o alejarlo** |
| "**First/best hover** para desambiguar varios interactuables" | El más cercano/mejor se queda el puntero | ✅ **Implementado hoy** (panel vs mesa vs HUD, gana el más cercano) — el mismo criterio de GZH-XR |
| "Tamaño mínimo de objetivo **48 dp**; **2,5°–3°** angular; **12 mm** de separación" | Los botones no pueden ser diminutos | ⚠️ Revisar los botones del HUD (la fuente bitmap 5×7 hace texto minúsculo) |
| "**Tracking loss suspende pointing/grabbing**" | Nunca se manda una orden sin tracking | ✅ `right_aim_valid` lo gatea |
| "Multimodal feedback: visual + **sonido + háptica**" | Confirmar cada selección | ✅ Tenemos audio y háptica |
| "Ergonomic neutrality: preferir interacción indirecta en sesiones largas" | El rayo cansa menos que tocar | ✅ Nuestro modelo es por rayo |
| "Tracking preciso a **milímetros**; estrategias para el ruido" | En AR de mesa, el puntero tiembla | ⚠️ El puntero necesita **snapping a la casilla** (ya hay rejilla magnética ✓) |
| "Usar **representaciones abstractas** en vez de realismo" | Elige lo que se lee | ✅ Casillas y iconos, no la escena isometric |

⇒ **El modelo de interacción que hemos construido es el correcto.** Los dos
ajustes: distancia del HUD y tamaño de los objetivos.

---

## 5. Las dos decisiones que cambian

### 5.1 El spike de GPU va desde **Termux**, no desde `adb shell`
El launcher del dispositivo (`scripts/start_factorio_client.sh`) se ejecuta hoy
como `shell` por adb. Para la GPU, tiene que ejecutarse **dentro de Termux**
(uid de app, con acceso a la GPU) o con el **wrapper de Vulkan de Android**.
Esto además resuelve el misterio de glvnd: en Termux la pila Mesa es **bionic**
(el `libEGL.so.1` de Termux es el de Android, sin despachador glvnd ni
manifiestos que reescribir) y el cliente glibc habla **GLX por el socket del
servidor X**, que es donde vive la GPU.

**Acción**: `scripts/start_factorio_termux.sh` (ya existe, `PISTA-TERMUX.md`)
pasa a ser la ruta principal de V1, y el payload de Mesa en `/data/local/tmp`
deja de ser necesario (Mesa lo pone Termux).

### 5.2 El HUD sale de la "middle distance"
El HUD holográfico está hoy en el borde de la mesa, a ~0,7 m y a media
distancia: la zona que las guías marcan como la peor para raycasting. Se sube a
la banda 0,9–1,2 m y se inclina hacia el jugador, siguiendo el "build window
tilted behind the board" de GZH-XR.

> **Revisado en el dispositivo (2026-10-24)**: el HUD **ya está** fuera de esa
> banda. Las ventanas del HUD nuevo (`src/ui/ventanas.rs`) se colocan en
> coordenadas absolutas de mundo a **1,30–1,60 m** de altura (y a 0,45–1,60 m
> de distancia), es decir, 0,6–0,9 m por encima de la mesa: el mismo criterio
> que el "build window tilted behind the board" de GZH-XR. **Este punto queda
> cerrado**; lo que queda es confirmarlo con los ojos en el visor.
>
> Lo que sí es "middle distance" es **la superficie de la mesa** (0,70 m), pero
> esa altura es la de la mesa real y con el anclaje espacial (`XR_BD_spatial_anchor`)
> es correcta por construcción: la fábrica se apoya donde se apoya tu mesa de verdad.

---

## 6. Cosas newfound que noये were on the plan

- **Los modelos 3D oficiales de Wube** (FFF-447, 25-09-2026) están **gratis en
  Printables**: 15 sets, 65 modelos, 247 STL, con la geometría real de las
  entidades (cintas, insertadores, horno, taladro, poste, cofres, biteros).
  Es una fuente de assets **oficial** que complementaría (o sustituiría) a los
  GLB de GDani31, que son recreaciones.
- **Factorio 2.1 ya es estable** (FFF-449, 09-10-2026) y la última es **2.1.23**
  (tenemos 2.1.22).
- **El "Ground View" de GZH-XR** (caminar por el mundo a escala humana sin dar
  órdenes) es un patrón potentísimo para un juego de fábrica: Built by the
  community's "look around the factory" idea. Es barato de añadir en el visor
  (cámara libre) y da la sensación de "estar dentro de la fábrica" que ninguna
  mesa da.

---

## 7. Veredicto por objetivo

| Objetivo | ¿Tiene sentido? |
|---|---|
| Jugar Factorio en la Pico 4 Ultra, MR, con la fábrica sobre mi mesa | **Sí**, y sin precedentes: es el único camino posible con el motor closed-source |
| Que el motor dibuje y el visor presente (V1) | **Sí**, pero el motor necesita GPU → **la vía es Termux**, no el shell |
| Que el visor dibuje el 3D (V2) | **Sí**, y es la red de seguridad si la GPU no cuadra |
| Quitar RCON y meter el ratón al motor | **Sí**, y además es lo correcto: el motor conserva sus reglas (fantasma, rotación, deshacer). No hay precedente, y no lo hay porque casi nadie tiene un motor al que inyectarle eventos |
| El enrutado "gana el más cercano" | **Sí**, validado por las guías de Meta y por GZH-XR |
| Anclaje a la mesa real (`XR_BD_spatial_anchor`) | **Sí**, es lo que hace GZH-XR ("Real surface"), con el mismo código de color (cian/naranja/rojo) |

**Lo que NO tiene sentido** (y por tanto se aparca): seguir peleando con el
payload de Mesa glibc en `/data/local/tmp` para conseguir GPU desde el shell;
y Optimize el FPS del motor como si fuera el objetivo (el objetivo es jugable,
y la GPU la aporta la app o Termux).

---

## 8. Referencias

- FFF-446 "An ARM and a Frame" — <https://factorio.com/blog/post/fff-446>
- FFF-447 "Factorio that you can touch" (modelos 3D oficiales) — <https://factorio.com/blog/post/fff-447>
- FFF-449 "2.1 Stable" — <https://factorio.com/blog/post/fff-449>
- Modelos oficiales: <https://www.printables.com/@WubeSoftware/models>
- GZH-XR (el proyecto de referencia): <https://github.com/Cesarus85/Generals-Zero-Hour-XR>
- GeneralsZH-Android (el port del motor que GZH-XR usa): <https://github.com/tarek369/GeneralsZH-Android>
- GeneralsVR (el juego por Quest Link): <https://github.com/Gonzorro/GeneralsVR>
- Termux, aceleración por hardware: <https://github.com/sabamdarif/termux-desktop/blob/main/docs/hw-acceleration.md>
- Droidspaces, GPU en contenedores: <https://github.com/ravindu644/Droidspaces-OSS/blob/main/Documentation/GPU-Acceleration.md>
- Android Vulkan Wrapper (PR mergeado en termux-packages): <https://github.com/termux/termux-packages/pull/22500>
- Android Vulkan Bridge (libhybris): <https://github.com/kde-yyds/android-vulkan-bridge>
- Meta: Hands UI best practices — <https://developers.meta.com/vr/design/hands-ui-best-practices/>
- Meta: Indirect interaction best practices — <https://developers.meta.com/vr/design/indirect_interactions_bp/>
- Microsoft Mixed Reality: Cursors — <https://learn.microsoft.com/en-au/windows/mixed-reality/design/cursors>
- Android XR / OpenXR: <https://developer.android.com/develop/xr/openxr/get-started>
- Nilsen, *Guidelines for the Design of Augmented Reality Strategy Games* —
  <http://www.meme-hazard.org/files/Nilsen--MScThesis-Guidelines-For-AR-Strategy-Games.pdf>
