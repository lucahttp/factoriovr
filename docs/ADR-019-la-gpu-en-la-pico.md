# ADR-019 — La GPU en la Pico: qué decide el kernel y qué decide el ICD

> **Estado**: implementado (sondas y barrido), **pendiente de medir en el
> dispositivo** (la Pico no estaba conectada por USB el 2026-10-24).
> Revisa: `ADR-013` (el motor dibuja), `ADR-015` (el cliente en la Pico).

---

## 1. El problema que había que cerrar

La variante V1 (el motor dibuja y el visor presenta) necesita que el cliente
gráfico de Factorio abra un contexto **OpenGL** dentro del visor. La cadena
es de dos capas y **cada capa falla por su cuenta**:

```text
Factorio (glibc)
   └─ libGL.so.1 (glvnd) ──► libEGL_mesa.so.0 ──► zink_dri.so ──► Vulkan
                                                                    └─ ICD
                                                                       ├─ turnip  → /dev/kgsl-3d0 → Adreno  (GPU real)
                                                                       └─ lavapipe → CPU                                   (software)
```

El nodo `/dev/kgsl-3d0` del visor es **666**, así que se abre. Pero que se
abra no dice nada de si se puede **reservar memoria de GPU**, que es lo que
hace Turnip en cada buffer.

## 2. El diagnóstico anterior era incorrecto

Se había concluded que el problema era el override del driver:

> `MESA_LOADER_DRIVER_OVERRIDE=kgsl` — el spike anterior usó `zink`, el
> override equivocado.

**Eso es falso, y se comprueba en los binarios del propio payload**
(`build/mesa/usr/lib/aarch64-linux-gnu/`):

| Binario | ¿Lee `MESA_LOADER_DRIVER_OVERRIDE`? |
|---|---|
| `libvulkan.so.1` (loader de Vulkan) | **NO** — la cadena no aparece en el binario |
| `libEGL_mesa.so.0` (vendor EGL) | **SÍ** (offset 778) |
| `dri/zink_dri.so` | **SÍ** (offset 83352) |

Consecuencias:

1. `MESA_LOADER_DRIVER_OVERRIDE` es el override del driver **DRI (capa GL)**.
   En Android no hay `/dev/dri/renderD*`, así que sin override el DRI no
   autodetecta nada y no encuentra `zink_dri.so`. El valor correcto para esa
   capa es **`zink`**.
2. El **ICD de Vulkan se elige aparte**, con `VK_ICD_FILENAMES` /
   `VK_DRIVER_FILES`. Como el loader de Vulkan **no lee** el override, no hay
   ningún conflicto entre las dos capas.
3. Además, este build de Mesa identifica a Turnip como **`freedreno`**
   (el binario contiene la cadena `freedreno` y `turnip`; **`kgsl` no
   aparece**, 0 ocurrencias). `kgsl` fue el nombre histórico del driver GL de
   freedreno (`kgsl_dri.so`), que **Mesa 23.x ya no trae** — el payload solo
   tiene `dri/zink_dri.so`.

O sea: `zink` era el override correcto y el spike estaba fallando por otra
cosa. La hipótesis se **convierte en medición** con el barrido (§5).

## 3. El riesgo de verdad: SELinux y la reserva de memoria

Precedente medido en otro Android con Adreno y SELinux enforcing
(DroidDeck #245, Quest 3S, Adreno 740v3 — la misma GPU que la Pico):

```text
chip_id=0x43050b00 gpu_id=740v3 gmem=3145728 mmu=1
plain-4K size=4096 -> FAIL errno=13 (Permission denied)
plain-64K size=65536 -> FAIL errno=13
plain-1M size=1048576 -> FAIL errno=13

avc: denied { ioctl } for path="/dev/kgsl-3d0" ioctlcmd=0x934
scontext=u:r:shell:s0 tcontext=u:r:object_r:gpu_device:s0 permissive=0
```

Es decir: la **consulta** del dispositivo (`IOCTL_KGSL_DEVICE_GETPROPERTY`)
funciona, pero el ioctl de **reserva** (`IOCTL_KGSL_GPUMEM_ALLOC_ID`, 0x934)
no. Turnip traduce el `EACCES` a `VK_ERROR_OUT_OF_DEVICE_MEMORY` y el
symptom es `vkCreateDevice failed (VkResult: -2)`.

El driver del fabricante no sufre ese problema porque reserva por el
**gralloc**, que es un HAL privilegiado.

**Esto es lo que hay que medir antes de tocar nada más**: si el kernel del
visor deniega la reserva, no hay ajuste de entorno que lo arregle.

## 4. La sonda: `scripts/probe_kgsl.c`

Pequeño binario **bionic** (ARM64 Android, se lanza tal cual con `adb shell`),
que separa las dos cosas:

1. **sysfs** (`gpu_model`, `gpu_chip_id`) — siempre legible.
2. `open("/dev/kgsl-3d0")` — ¿se abre?
3. `IOCTL_KGSL_DEVICE_GETPROPERTY` (`KGSL_PROP_DEVICE_INFO`) — la consulta.
4. `IOCTL_KGSL_GPUMEM_ALLOC_ID` con 4K, 64K y 1M — **la reserva**.
5. `IOCTL_KGSL_GPUOBJ_ALLOC` — la otra familia (en el Quest 3S esta sí
   estaba permitida).

Las estructuras están **copiadas verbatim** de la cabecera UAPI real
(`include/uapi/linux/msm_kgsl.h`, KGSL 3.14), porque **el tamaño de cada
`struct` va codificado dentro del número de ioctl** (`_IOC` codifica el
`size`): si la `struct` no coincide con la del kernel, el kernel responde
`ENOTTY` y **no hay ningún error de compilación que lo avise**. Por eso hay
`_Static_assert` de tamaño.

Los números de ioctl que genera, comprobados contra la referencia
independiente del Quest 3S (`0x902` y `0x934`):

| ioctl | número | tamaño |
|---|---|---|
| `IOCTL_KGSL_DEVICE_GETPROPERTY` | `0xc0180902` | 24 |
| `IOCTL_KGSL_GPUMEM_ALLOC_ID` | `0xc0300934` | 48 |
| `IOCTL_KGSL_GPUMEM_FREE_ID` | `0xc0100935` | 16 |
| `IOCTL_KGSL_GPUOBJ_ALLOC` | `0xc0300945` | 48 |

## 5. El barrido: `scripts/spike_turnip_pico.sh`

Dos partes, en este orden (la primera decide si la segunda tiene sentido):

1. **Lo que deja hacer el kernel**: se compila y sube la sonda y se ejecuta.
2. **Lo que logra abrir el motor**: se lanza el cliente gráfico real con
   cuatro combinaciones y se lee su línea `Initialised OpenGL`, que dice el
   renderer con el que ha abierto el contexto.

| Caso | ICD | `MESA_LOADER_DRIVER_OVERRIDE` | Para qué |
|---|---|---|---|
| A | turnip | `zink` | el candidato real |
| B | lavapipe | `zink` | **control**: prueba que la cadena GL funciona |
| C | turnip | `kgsl` | comprueba la hipótesis descartada (§2) |
| D | turnip | `zink` + `GALLIUM_DRIVER=zink` | variante sin override del ICD |

El caso B es el que da el diagnóstico: **si B falla, el problema es la cadena
GL/EGL; si B va bien y A no, es el ICD** (y entonces lo mira §3).

Para que el barrido pueda imponer el ICD, `scripts/start_factorio_client.sh`
ahora **respeta** el `VK_ICD_FILENAMES` que le llegue puesto, en vez de
imponerlo él.

## 6. Lo que NO ha podido medirse (y por qué)

La Pico no estaba conectada por USB el 2026-10-24 (el cable se ha soltado
tres veces). No hay red interna alternativa: `adb` no ve ningún dispositivo,
no hay ADB abierto en la red local (`5555`, `5556`, `37099` cerrados en los
23 hosts que responden ARP) ni aparece por mDNS (`_adb._tcp`).

Así que **el barrido está escrito y compilado, pero sin ejecutar**. La
primerameasurement en cuanto vuelva el cable es:

```bash
bash scripts/spike_turnip_pico.sh
```

## 7. Efecto colateral: el banco del inyector ya no puede validar

Al revisar el banco `scripts/test_injector.sh` se descubrió que **el motor se
cuelga bajo render por software**, y **no es culpa del inyector**:

```text
# Con el inyector:  se queda en "Parallel sprite loader initialized" (6.9 s)
# SIN el inyector:  exactamente lo mismo, al 195 % de CPU
```

El control (el mismo comando sin `LD_PRELOAD`) se cuelga igual, así que el
problema es del motor bajo zink+lavapipe en este contenedor, no del parche.
Como el hook de `GameRenderer::render` **solo corre cuando el motor dibuja
el mundo**, la ausencia de frames no dice nada del inyector.

Por eso el banco ahora distingue los dos casos, siguiendo la convención del
proyecto (0 ok, 1 fallo real, 2 no validable):

- el motor **no** llegó a `Factorio initialised` → **código 2** (no validable),
  con el diagnóstico (se quedó cargando el atlas por software);
- el motor **sí** inicializó y dibujó, y no hay frames → **código 1**
  (el hook está muerto: eso sí es un fallo real).

## 8. Decisiones que se quedan

| # | Decisión | Por qué |
|---|---|---|
| 1 | El ICD de Vulkan se elige **siempre** con `VK_ICD_FILENAMES`, nunca con `MESA_LOADER_DRIVER_OVERRIDE` | el loader de Vulkan no lee esa variable (verificado en el binario) |
| 2 | `MESA_LOADER_DRIVER_OVERRIDE=zink` para la capa GL | en Android no hay `renderD*` y el DRI no autodetecta; `kgsl` ya no existe en Mesa 23.x |
| 3 | La GPU del visor se decide con **una medida** (la sonda de ioctls), no con ajustes | si SELinux deniega la reserva no hay arreglo sin root, y conviene saberlo ya |
| 4 | Turnip se sigue preferring a `vulkan.adreno.so` del fabricante | el del fabricante es bionic y reventaría dentro del proceso glibc del motor (dos libc en el mismo proceso) |
| 5 | La ruta sin root **no es** la del driver del fabricante | el driver de Adreno reserva por gralloc (HAL privilegiado); solo funciona en apps del sistema |

## 9. Qué queda

1. Con la Pico conectada: `bash scripts/spike_turnip_pico.sh`.
   - Si la sonda dice **GPU UTILIZABLE** → seguir con el barrido y ajustar el
     entorno hasta que el motor abra el contexto sobre el Adreno.
   - Si dice **GPU PARCIAL** (consulta sí, reserva no) → cerrar la variante
     V1 en el visor y quedarse con **V2** (el visor dibuja desde el estado,
     que ya funciona a 90 FPS), documentando el motivo.
2. Si hace falta, el **Android Vulkan Wrapper** (`mesa-vulkan-icd-wrapper`),
   que delega la GPU a un proceso privilegiado: es la vía que queda si
   Turnip no puede reservar memoria.
3. Cerrar el doble disparo (XFCE + RCON) y el arranque de Minecraft PE (§10
   de `ADR-016`).
