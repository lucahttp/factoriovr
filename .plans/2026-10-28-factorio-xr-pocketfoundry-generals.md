# Plan: Factorio XR Inmersivo en Pico 4 Ultra (Pocket Foundry + Generals XR + Factorio 3D Mod)

> **Fecha:** 2026-10-28  
> **Objetivo:** Experiencia completa de Factorio en Realidad Mixta sobre Pico 4 Ultra combinando el runtime autónomo de Pocket Foundry, la interacción espacial/tabletop de Generals Zero Hour XR y los modelos 3D volumétricos con FPV de Factorio 3D Mod.

---

## 1. Arquitectura del Sistema

El sistema integra las tres mejores soluciones del ecosistema en una única aplicación autónoma (APK) para la Pico 4 Ultra:

```
┌────────────────────────────────────────────────────────────────────────┐
│                   XFACTORVR TABLETOP & FPV ENGINE                      │
├────────────────────────────────┬───────────────────────────────────────┤
│    PICO 4 ULTRA OPENXR MOTOR   │         POCKET FOUNDRY RUNTIME        │
│    (Estilo Generals XR)        │       (Factorio Linux ARM64 Nativo)   │
├────────────────────────────────┼───────────────────────────────────────┤
│ • Loader PICO_SDK              │ • Ejecución directa sin proot         │
│ • Perfiles pico4s_controller   │ • glibc 2.44 parcheada para Android   │
│ • Mesa Espacial + Passthrough  │ • Mesa Zink + Turnip (WSI fence fix)  │
│ • Gestos bimanuales (Pan/Zoom) │ • SDL3_DYNAMIC_API (/path/libSDL3_xr) │
│ • Puntero láser + Raycast 3D   │ • Audio float32 48kHz (FAB1 socket)   │
│ • Render estéreo GLB + Sombras │ • Gamepad virtual Xbox + mouse        │
├────────────────────────────────┴───────────────────────────────────────┤
│                         FACTORIO 3D MOD (GDani31)                      │
├────────────────────────────────────────────────────────────────────────┤
│ • Modelos GLB (ensambladoras, hornos, cintas, insertadores animados)   │
│ • Modo Tabletop (diorama sobre la mesa física en Passthrough)         │
│ • Modo FPV (inmersión en primera persona escala 1:1)                  │
│ • GUI nativa y mouse picking integrados sin perder menús del juego     │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Priorización por Riesgo (Risk-First)

1. **Riesgo 1 (Máximo): Runtime Pocket Foundry en Pico 4 Ultra**
   - Validar que el bundle de glibc 2.44 + Turnip + Zink de Pocket Foundry corra en el Snapdragon XR2 Gen 2 (Adreno 740) de la Pico sin proot ni Termux.
2. **Riesgo 2 (Alto): Puente `SDL3_DYNAMIC_API` unificado**
   - Implementar la biblioteca `libSDL3_xr.so` que capture los frames de renderizado (swap window), reciba los inputs del motor OpenXR (gamepad + láser) y streamee el audio.
3. **Riesgo 3 (Medio): Diorama Tabletop y Controles Generals XR en Pico OS**
   - Adaptar el loop OpenXR con `PICO_SDK/libopenxr_loader.so` y `pico4s_controller` con gestos de arrastre y escalado de la mesa.
4. **Riesgo 4 (Medio): Renderizado 3D y Transición a FPV**
   - Proyectar los modelos GLB sobre la mesa y permitir el zoom fluido hasta escala 1:1 (First Person View).

---

## 3. Tabla de Progreso y Tareas

| ID | Tarea | Estado | Prioridad | Notas |
|---|---|---|---|---|
| **T1** | Analizar e importar el runtime y parches de Pocket Foundry (`glibc`, `Mesa`, `SDL3`) | 🟢 Completado | P0 (Crítico) | APK debug compila con éxito (34MB) con runtime Turnip+Zink |
| **T2** | Construir puente de Gamepad Xbox + Audio `FAB1` + Frame Hook | 🟢 Completado | P0 (Crítico) | Integrado nativo en Pocket Foundry (`Bridges.kt` + `SDL_pocketfoundry.c`) |
| **T3** | Integrar cargador OpenXR Pico 4 Ultra (`libopenxr_loader.so`) y perfiles `pico4s_controller` | 🟢 Completado | P1 (Alto) | Empaquetado en `jniLibs/arm64-v8a`, queries y permisos en AndroidManifest |
| **T4** | Implementar la Mesa Espacial (Tabletop) en Passthrough con gestos bimanuales (Pan, Scale, Rotate) | 🟡 En progreso | P1 (Alto) | Enlazando `SurfaceTexture` de `LorieView` con `openxr_engine.cpp` |
| **T5** | Integrar los modelos GLB de `factorio-3d-mod` en la grilla espacial de la mesa | 🟢 Completado | P2 (Medio) | 26 modelos GLB + metadatos y shaders empaquetados en `assets/` |
| **T6** | Implementar la transición fluida a Primera Persona (FPV) al hacer zoom máximo | ⚪ Pendiente | P2 (Medio) | Navegación 1:1 dentro de la fábrica |
| **T7** | Empaquetar el APK Standalone All-In-One para Pico 4 Ultra y desplegar | 🟡 En progreso | P1 (Alto) | APK debug listo en `/Volumes/X10Pro/Projects/pocket-foundry/app/build/outputs/apk/debug/app-debug.apk` |

---

## 4. Decisiones de Diseño y DX

1. **Documentación Primero (DX First):**
   - Toda interfaz de comunicación entre el motor OpenXR y el runtime de Factorio usará sockets Unix locales (`/data/data/com.xfactorvr/files/tmp/`) con protocolos binarios mínimos y bien documentados.
2. **Cero Dependencia de Termux:**
   - La aplicación final se empaqueta como un APK único autoportante (`XFactorVR.apk`). Al abrirse en la Pico, inicializa su runtime interno y arranca directamente en Realidad Mixta.
3. **Preservación del Juego Nativo:**
   - Se mantiene la lógica oficial del binario de Factorio, su soporte multijugador, mods oficiales y guardado de partidas.
