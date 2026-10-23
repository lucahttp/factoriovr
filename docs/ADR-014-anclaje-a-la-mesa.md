# ADR-014 — Anclaje de la fábrica a la mesa física real

- **Estado**: aceptada (implementada en `android/src/ancla_espacial.{h,cpp}`)
- **Fecha**: 2026-10-22
- **Ámbito**: visor nativo OpenXR (APK Android ARM64) para Pico 4 Ultra

---

## 1. Contexto y problema

Hoy el visor coloca el diorama de Factorio en el **espacio de referencia STAGE**
de OpenXR, con una altura fija (≈ 0,74 m) y un yaw calculado a partir de hacia
donde mira el usuario. El resultado es que la fábrica flota en un plano
imaginario a la altura del pecho: no está anclada a la mesa real, así que
cualquier movimiento de la cabeza "despega" la fábrica de la mesa y el usuario
no puede trabajar sobre ella como si fuera un tablero de verdad.

El objetivo es que **la fábrica aparezca tumbada sobre la mesa física que el
visor detecta**, con la misma persistencia en el mundo real que un ancla
spatial.

Restricciones del entorno:

- Runtime **OpenXR 1.1** de PICO OS ≥ 5.12.0U (Pico 4 Ultra).
- El APK es una app OpenXR nativa en C++ con el loader estándar de Khronos
  (`android/lib/arm64-v8a/libopenxr_loader.so`).
- Extensiones relevantes disponibles en ese runtime: `XR_BD_spatial_anchor`,
  `XR_BD_spatial_anchor_persistence` (alias), `XR_BD_spatial_mesh`,
  `XR_BD_spatial_plane`, `XR_BD_spatial_scene`, `XR_FB_passthrough` (ya usada
  para el color de fondo).
- El módulo debe ser **autónomo**: no puede tocar `openxr_engine.cpp`,
  `scripts/build_apk.sh` ni otros ficheros del visor.

---

## 2. Decisión

**Usar las extensiones de sensing espacial de ByteDance (`XR_BD_spatial_sensing`
+ `XR_BD_spatial_anchor` + `XR_BD_spatial_plane`) mediante un módulo nuevo,
`android/src/ancla_espacial.{h,cpp}`, que expone una API C++ propia
(`namespace xfv`) y degrada con elegancia al STAGE si el runtime no ofrece
las extensiones.**

La API pública del módulo es:

| Función | Para qué sirve |
|---|---|
| `ancla_espacial_iniciar(instancia, sesion, sistema)` | Comprueba extensiones, carga punteros a función y deja el módulo activo o inactivo. |
| `ancla_espacial_disponible()` | ¿Está el módulo activo? |
| `ancla_espacial_preparar_mesa()` | Arranca la captura de superficies (el usuario ya ha confirmado la mesa). |
| `ancla_espacial_confirmar_mesa()` | Fuerza el anclaje sobre la superficie detectada ahora mismo. |
| `ancla_espacial_actualizar(frame)` | Bomba las operaciones asíncronas y actualiza la pose del ancla. |
| `ancla_espacial_matriz(out16)` | Matriz column-major de la pose del ancla en STAGE (identidad si no hay ancla). |
| `ancla_espacial_persistir(etiqueta)` / `ancla_espacial_restaurar(etiqueta)` | Guardar / recuperar la mesa entre sesiones. |
| `ancla_espacial_estado()` | Texto corto para el log: `"sin extension"`, `"esperando mesa"`, `"anclado"`, `"persistido"`, `"mesa perdida"`… |
| `ancla_espacial_parametros(&escala, &altura)` | Escala 1:1 y despegue del diorama. |

### 2.1 Por qué `XR_BD_spatial_*` y no otras

| Alternativa | Por qué no |
|---|---|
| `XR_FB_passthrough` (ya en uso) | Solo da el color de fondo de la cámara; no da geometría del entorno ni superficies. |
| ARCore / OKPose (anchor por imagen/Vuforia) | La app es OpenXR pura y no tiene acceso a la cámara de ARCore sin duplicar el flujo de cámara de la Pico; además requiere Google's Play Services, que no están en el APK standalone. |
| `XR_MSFT_spatial_anchor` / `XR_EXT_spatial_anchor` | No están soportados por el runtime de PICO. |
| `XR_BD_spatial_mesh` | La malla es pesada y no garantiza una superficie plana "de mesa"; sirve como plan B, no como vía principal. |
| Queda en STAGE con altura fija (lo actual) | Es exactamente el problema que se quiere resolver. |
| Estimar la mesa por la altura de la vista | No es la mesa real: es la conjetura del visor. Sirve solo como fallback. |
| **XR_BD_spatial_plane + XR_BD_spatial_anchor** | **Elegida**: superficies planas reales detectadas por el runtime, con ancla persistente y pose continua en el espacio STAGE. |

---

## 3. Cómo se detecta la mesa (flujo interno del módulo)

1. **Comprobación de extensiones** (`ancla_espacial_iniciar`):
   `xrEnumerateInstanceExtensionProperties` y se buscan
   `XR_BD_spatial_sensing`, `XR_BD_spatial_anchor`, `XR_BD_spatial_plane`
   (y `XR_BD_spatial_scene` como alternativa). Si faltan las dos primeras, el
   módulo queda **inactivo** y el visor sigue en STAGE.
2. **Carga de la ABI**: como el `openxr.h` vendorizado (1.1.45) **no**
   incluye las extensiones BD, el módulo transcribe en el `.cpp` la ABI
   mínima que necesita (ver §6) y carga cada función con
   `xrGetInstanceProcAddr`. Si el runtime no expone alguna, el módulo se
   queda inactivo.
3. **Propiedades del sistema**: `xrGetSystemProperties` encadenando
   `XrSystemSpatialSensingPropertiesBD` y `XrSystemSpatialAnchorPropertiesBD`
   para confirmar que el *sistema* soporta sensing y anchors.
4. **Espacio STAGE propio**: el módulo crea su propio `XrSpace` de tipo
   `STAGE` (con fallback a `LOCAL`) para poder expresar la pose del ancla en
   el mismo sistema de referencia que usa el visor.
5. **Captura de superficies** (`ancla_espacial_preparar_mesa`): se crean y
   arrancan dos *sense data providers*:
   - `PLANE` (o `SCENE` si no hay planos), para detectar superficies;
   - `ANCHOR`, para poder crear el ancla sobre la entidad detectada.
   El arranque es asíncrono (`xrStartSenseDataProviderAsyncBD` +
   `...CompleteBD`) y se resuelve frame a frame en `ancla_espacial_actualizar`.
6. **Consulta periódica** (cada ~8 frames):
   `xrQuerySenseDataAsyncBD` con el filtro
   `XrSenseDataFilterPlaneOrientationBD` = `HORIZONTAL_UPWARD` (solo mesas y
   suelos, nada de paredes ni techos). Con el snapshot resultante:
   - `xrGetSpatialEntityComponentDataBD` con
     `XrSpatialEntityLocationGetInfoBD` → **posición de la superficie ya
     expresada en nuestro espacio STAGE** (así no hay que componer poses a
     mano);
   - componente `PLANE_ORIENTATION` → confirma que es horizontal hacia arriba;
   - componente `POLYGON` (o `BOUNDING_BOX_2D` como reserva) → tamaño
     aproximado en metros.
7. **Filtro de "esto es una mesa"**: semi-extensión entre **0,15 m y 2 m**,
   altura sobre el suelo entre **0,1 m y 2 m**, orientación horizontal hacia
   arriba. De los candidatos válidos se elige **el más grande**.
8. **Estabilidad**: la misma entidad debe ganar 3 consultas seguidas; entonces
   se crea el ancla (`xrCreateSpatialEntityAnchorBD` →
   `xrCreateAnchorSpaceBD`). También se puede forzar antes con
   `ancla_espacial_confirmar_mesa()` (el usuario confirma "aquí está la mesa").
9. **Pose continua**: cada frame,
   `xrLocateSpace(espacioAnchor, espacioStage, predictedDisplayTime)`; si la
   posición y la orientación son válidas y están *trackeadas*, se actualiza la
   matriz; si se pierden más de 30 frames seguidos, el ancla se destruye y el
   módulo vuelve a "mesa perdida" (y a la identidad).

### 3.1 Composición de matrices

`ancla_espacial_matriz()` devuelve la **pose del ancla en STAGE**, en
column-major como el resto del visor (traslación en `m[12..14]`). El visor
compone:

```
final = ancla_matriz * (rot_y(yaw_local) * escala * desplazamiento_local)
```

La pose detectada aporta la posición y la orientación base de la mesa real;
el yaw local, la escala 1:1 y el despegue los pone el visor. Mientras no haya
ancla, el visor usa su `model_mesa` de siempre (STAGE, 0,74 m).

---

## 4. Escala 1:1 y altura del diorama

- **1 casilla de Factorio = 32 px** en la imagen del motor.
- **1 casilla = 0,04 m** (4 cm) reales sobre la mesa.
  → `xfv::kEscalaCasillaM = 0.04f` (y `ancla_espacial_escala_casilla()`).
- **Despegue sobre la superficie: 0,002 m (2 mm)**
  → `xfv::kAlturaDioramaM = 0.002f`, para que la base del diorama no haga
  *z-fighting* con la superficie detectada.
- El diorama se dibuja **tumbado**: su plano XZ es el plano de la mesa y la
  altura (Y) crece hacia el usuario.

Para una mesa de 1,2 × 0,7 m, el diorama 1:1 ocupa 30 × 17,5 casillas.

---

## 5. Persistencia

La persistencia de `XR_BD_spatial_anchor` es **por UUID**, no por nombre:

1. `xrPersistSpatialAnchorAsyncBD` con
   `XrSpatialAnchorPersistInfoBD { location = XR_PERSISTENCE_LOCATION_LOCAL_BD,
   anchor }` y su `...CompleteBD` equivalente (se resuelve en el frame loop).
2. El UUID del ancla se obtiene con `xrGetAnchorUuidBD` y se guarda en la
   tabla **etiqueta → UUID** del módulo.
3. La tabla se persiste en un fichero de texto (una línea
   `etiqueta uuid_hex` por ancla) si el visor llama a
   `ancla_espacial_fijar_ruta_persistencia(ruta)`. Sin ruta, la tabla vive
   solo en memoria y se pierde al cerrar la app (limite conocido, documentado).
4. `ancla_espacial_restaurar(etiqueta)` busca el UUID, consulta al provider
   de anchors con el filtro `XrSenseDataFilterUuidBD`, localiza la entidad y
   vuelve a crear el ancla y su espacio.

> Nota: `XR_BD_spatial_anchor_persistence` no es una extensión separada en el
> registro de Khronos; la persistencia vive dentro de `XR_BD_spatial_anchor`.
> El módulo acepta ambos nombres al comprobar extensiones, por si el runtime
> de PICO anuncia la persistencia por separado.

---

## 6. El problema de los headers (y cómo se resuelve)

El `android/include/openxr/openxr.h` del repo es el **1.1.45 de Khronos** y
**no contiene** ninguna definición de `XR_BD_spatial_*` (ni en `openxr.h` ni
en `openxr_reflection.h`), pese a que esas extensiones **sí** están en el
registro oficial de Khronos (`specification/registry/xr.xml`, extensiones 390
`XR_BD_spatial_sensing`, 391 `XR_BD_spatial_anchor`, 393 `XR_BD_spatial_scene`,
397 `XR_BD_spatial_plane`).

Para no regenerar ni tocar los headers del repo (que no son de este worker),
el módulo **transcribe en su `.cpp`** la ABI mínima que usa, con:

- las **disposiciones de memoria exactas** del header oficial, y
- los **valores numéricos exactos** de `XrStructureType`/`XrResult`,
  obtenidos generando el header con el generador oficial de Khronos
  (`scripts/genxr.py` sobre `xr.xml`).

Ejemplos: `XR_TYPE_SPATIAL_ANCHOR_CREATE_INFO_BD = 1000390001`,
`XR_TYPE_SENSE_DATA_PROVIDER_CREATE_INFO_BD = 1000389009`,
`XR_SENSE_DATA_PROVIDER_TYPE_PLANE_BD = 1000396000`,
`XR_ERROR_FUTURE_PENDING_EXT = -1000469001`.

Cuando el repo actualice sus headers, ese bloque (`namespace xfv::bd`) puede
borrarse sin tocar el resto del módulo.

---

## 7. Qué pasa si no hay mesa (o no hay extensiones)

| Situación | `ancla_espacial_estado()` | `ancla_espacial_disponible()` | `ancla_espacial_matriz()` |
|---|---|---|---|
| El runtime no anuncia las extensiones | `"sin extension"` | false | **Identidad** |
| Las anuncia pero no están habilitadas / fallan | `"extension inactiva"` | false | **Identidad** |
| Módulo listo, todavía no se pide detectar | `"listo"` | true | **Identidad** |
| Buscando la superficie | `"esperando mesa"` | true | **Identidad** |
| Creando el ancla | `"anclando"` | true | **Identidad** |
| Ancla viva y trackeada | `"anclado"` / `"persistido"` | true | **Pose del ancla** |
| Se perdió el tracking > 30 frames | `"mesa perdida"` | true | **Identidad** |

Reglas duras:

- `ancla_espacial_matriz()` **siempre** escribe 16 floats válidos (identidad
  como mínimo). Nunca deja NaNs ni la matriz sin inicializar.
- `ancla_espacial_actualizar()` devuelve `false` si no hay ancla válida; el
  visor sigue con su ruta STAGE habitual.
- Ninguna llamada OpenXR del módulo aborta el bucle de render: los errores se
  registran en el log con la etiqueta `xfv-ancla` y se degradan a "sin ancla".

---

## 8. Requisito pendiente: habilitar las extensiones en la instancia

`XR_BD_spatial_*` son **extensiones de instancia**. El motor crea la instancia
OpenXR en `android/src/openxr_engine.cpp`; para que el runtime las soporte hay
que pedirlas explícitamente en la lista de extensiones de `xrCreateInstance`:

```
"XR_BD_spatial_sensing", "XR_BD_spatial_anchor", "XR_BD_spatial_plane"
```

Si no se piden, el runtime las anuncia pero devuelve `XR_ERROR_FEATURE_UNSUPPORTED`
al usarlas; el módulo lo detecta y queda en `"extension inactiva"`, sin romper
nada. **Este cambio está fuera del alcance de este módulo** (lo hará el worker
que integre el visor), pero es la condición necesaria para que la detección
de mesa funcione en el dispositivo.

---

## 9. Cómo se valida en el dispositivo

1. **Compilación**: el módulo se compila con el mismo clang++ de la NDK que
   usa `scripts/build_apk.sh` (verificado con `-fsyntax-only -Wall -Wextra`).
2. **En la Pico** (Pico 4 Ultra, PICO OS ≥ 5.12.0U, visor con la app instalada):
   - `adb logcat -s xfv-ancla` para ver el log del módulo.
   - Al arrancar la sesión debe aparecer
     `Extensiones: sensing=1 anchor=1 plane=1 scene=1 persist=1` y
     `Anclaje espacial activo`.
   - Si aparece `Sin XR_BD_spatial_sensing/anchor`, el runtime no los expone:
     revisar la versión de PICO OS y que la app pida las extensiones.
3. **Prueba funcional**:
   - Colocar la Pico frente a la mesa, mirar a la mesa y pulsar la tecla de
     "fijar mesa" (llamada a `ancla_espacial_preparar_mesa()`).
   - En el log debe aparecer `Superficie candidata <id> (radio X.XX m)` y, a los
     pocos frames, `Ancla creada sobre la superficie <id>` y el estado pasa a
     `"anclado"`.
   - **Prueba clave**: la fábrica debe quedarse pegada a la mesa al girar la
     cabeza y al agacharse; si se mueve con la vista, es que no hay ancla y se
     está usando el STAGE.
   - Al alejar el visor de la mesa (perder tracking) el estado debe pasar a
     `"mesa perdida"` y la fábrica volver al STAGE **sin petar**.
   - `ancla_espacial_persistir("mesa")` → log `Ancla 'mesa' persistida (<uuid>)`;
     reiniciar la app y `ancla_espacial_restaurar("mesa")` → `Ancla 'mesa'
     restaurada`.
4. **Prueba de degradación**: arrancar en un runtime sin extensiones (o
   deshabilitarlas en la creación de la instancia) → el visor debe seguir
   mostrando la tabla en STAGE a 0,74 m, sin errores ni NaNs.

---

## 10. Límites conocidos

- La calidad de la detección depende del runtime de PICO: si el
  `sense data provider` de planos no está disponible en PICO OS 5.12, hay que
  recurrir a `XR_BD_spatial_scene` (ya está contemplado como alternativa) o a
  un anclaje manual por punto.
- El módulo no implementa suavizado (filtering) de la pose del ancla: la
  estabilidad viene de las 3 detecciones seguidas. Si el runtime entrega poses
  con jitter, puede hacer falta un filtro en el visor.
- La tabla de persistencia es texto plano en el directorio de la app; si se
  quiere cifrar o respaldar, es otro worker.
- El módulo **no** decide el yaw de la fábrica: solo aporta la pose de la
  mesa. El yaw local sigue siendo del visor.

---

## 11. Ficheros de este ADR

- `android/src/ancla_espacial.h` — API documentada en español.
- `android/src/ancla_espacial.cpp` — implementación (incluye la transcripción
  de la ABI `XR_BD_spatial_*`).
- `docs/ADR-014-anclaje-a-la-mesa.md` — este documento.
