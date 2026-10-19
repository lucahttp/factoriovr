// launcher.cpp — Lanzador nativo Android NativeActivity para Pico 4 Ultra
//
// Inicializa el contexto de Pico OS, carga el cargador OpenXR (libopenxr_loader.so),
// configura el entorno de ejecución glibc/proot y arranca Factorio con el mod 3D inyectado.

#include <android/native_activity.h>
#include <android/window.h>
#include <android/log.h>
#include <stdlib.h>
#include <unistd.h>
#include <dlfcn.h>
#include <string.h>
#include <stdio.h>
#include <sys/stat.h>

#define LOG_TAG "XFactorVR_Launcher"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

// Configuración de rutas internas en Pico 4 Ultra
static const char* DATA_DIR = "/sdcard/XFactorVR";
static const char* MOD_LIB = "libxfactorvr.so";

static void ensure_directories_exist() {
    struct stat st = {0};
    if (stat(DATA_DIR, &st) == -1) {
        mkdir(DATA_DIR, 0777);
        LOGI("Creado directorio de datos: %s", DATA_DIR);
    }
}

static void setup_environment(ANativeActivity* activity) {
    // 1. Configurar LD_PRELOAD para inyectar nuestra capa 3D en el motor
    char preload_path[512];
    snprintf(preload_path, sizeof(preload_path), "%s/lib/%s", activity->internalDataPath, MOD_LIB);
    setenv("LD_PRELOAD", preload_path, 1);
    LOGI("LD_PRELOAD configurado: %s", preload_path);

    // 2. Configurar rutas de librerías nativas y datos
    setenv("FACTORIO_PATH", DATA_DIR, 1);
    setenv("OPENXR_RUNTIME", "PicoOpenXR", 1);
    setenv("PVR_SEE_THROUGH", "1", 1); // Forzar habilitación de Passthrough en Pico OS
}

#include "openxr_engine.h"

static void onStart(ANativeActivity* activity) {
    LOGI("ANativeActivity onStart");
}

static void onResume(ANativeActivity* activity) {
    LOGI("ANativeActivity onResume");
}

static void onPause(ANativeActivity* activity) {
    LOGI("ANativeActivity onPause");
}

static void onStop(ANativeActivity* activity) {
    LOGI("ANativeActivity onStop");
}

static void onWindowFocusChanged(ANativeActivity* activity, int hasFocus) {
    LOGI("ANativeActivity onWindowFocusChanged: %d", hasFocus);
}

static void onConfigurationChanged(ANativeActivity* activity) {
    LOGI("ANativeActivity onConfigurationChanged");
}

static void onLowMemory(ANativeActivity* activity) {
    LOGI("ANativeActivity onLowMemory");
}

static void onNativeWindowCreated(ANativeActivity* activity, ANativeWindow* window) {
    LOGI("ANativeWindow creada: %p", window);
    ANativeActivity_setWindowFlags(activity,
        AWINDOW_FLAG_FULLSCREEN | AWINDOW_FLAG_KEEP_SCREEN_ON,
        0);
}

#include <android/input.h>

static AInputQueue* s_input_queue = nullptr;

static void onInputQueueCreated(ANativeActivity* activity, AInputQueue* queue) {
    LOGI("ANativeActivity onInputQueueCreated: %p", queue);
    s_input_queue = queue;
}

static void onInputQueueDestroyed(ANativeActivity* activity, AInputQueue* queue) {
    LOGI("ANativeActivity onInputQueueDestroyed: %p", queue);
    s_input_queue = nullptr;
}

void pump_android_input_queue() {
    if (!s_input_queue) return;
    AInputEvent* event = nullptr;
    while (AInputQueue_getEvent(s_input_queue, &event) >= 0) {
        if (AInputQueue_preDispatchEvent(s_input_queue, event)) {
            continue;
        }
        AInputQueue_finishEvent(s_input_queue, event, 0);
    }
}

static void onNativeWindowDestroyed(ANativeActivity* activity, ANativeWindow* window) {
    LOGI("ANativeWindow destruida: %p", window);
}

static void onDestroy(ANativeActivity* activity) {
    LOGI("ANativeActivity onDestroy");
    stop_openxr_thread();
}

// Callback de inicio de NativeActivity
extern "C" void ANativeActivity_onCreate(ANativeActivity* activity, void* savedState, size_t savedStateSize) {
    LOGI("=== Iniciando XFactorVR Tabletop en Pico 4 Ultra ===");

    // Registrar callbacks para ciclo de vida Android
    activity->callbacks->onStart = onStart;
    activity->callbacks->onResume = onResume;
    activity->callbacks->onPause = onPause;
    activity->callbacks->onStop = onStop;
    activity->callbacks->onDestroy = onDestroy;
    activity->callbacks->onWindowFocusChanged = onWindowFocusChanged;
    activity->callbacks->onNativeWindowCreated = onNativeWindowCreated;
    activity->callbacks->onNativeWindowDestroyed = onNativeWindowDestroyed;
    activity->callbacks->onInputQueueCreated = onInputQueueCreated;
    activity->callbacks->onInputQueueDestroyed = onInputQueueDestroyed;
    activity->callbacks->onConfigurationChanged = onConfigurationChanged;
    activity->callbacks->onLowMemory = onLowMemory;

    ensure_directories_exist();
    setup_environment(activity);

    // Cargar la librería del mod de Realidad Mixta
    void* mod_handle = dlopen(MOD_LIB, RTLD_NOW | RTLD_GLOBAL);
    if (!mod_handle) {
        LOGE("No se pudo cargar %s: %s", MOD_LIB, dlerror());
    } else {
        LOGI("Módulo XFactorVR OpenXR cargado exitosamente");
        typedef void (*init_fn_t)();
        init_fn_t xfactorvr_init = (init_fn_t)dlsym(mod_handle, "xfactorvr_init");
        if (xfactorvr_init) {
            LOGI("Llamando a xfactorvr_init()...");
            xfactorvr_init();
            LOGI("xfactorvr_init() ejecutado correctamente");
        } else {
            LOGE("Símbolo xfactorvr_init no encontrado en %s: %s", MOD_LIB, dlerror());
        }
    }

    // Arrancar el motor y sesión estéreo OpenXR para Pico OS
    start_openxr_thread(activity);

    LOGI("Entorno listo. La simulación de Factorio y el Tabletop OpenXR están activos.");
}
