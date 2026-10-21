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
#include <dirent.h>
#include <errno.h>
#include <thread>
#include <string.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <fcntl.h>

#define LOG_TAG "XFactorVR_Launcher"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

// Configuración de rutas internas en Pico 4 Ultra
static const char* DATA_DIR = "/sdcard/XFactorVR";
static const char* MOD_LIB = "libxfactorvr.so";

// Motor real de Factorio dentro del visor.
// /sdcard esta montado como noexec, asi que el motor vive en /data/local/tmp.
static const char* START_SCRIPT = "/data/local/tmp/start_factorio_standalone.sh";
// Escenario de campana (el nivel de tutorial parcheado para multijugador).
// Lo deja el despliegue en el data del motor; si esta, el servidor arranca
// con el y no con una partida guardada.
static const int   RCON_PORT = 27015;

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

// ============================================================================
// ARRANQUE DEL SERVIDOR FACTIO (modo standalone)
// ============================================================================
// La app es un cliente RCON: el estado de la mesa viene del motor real. Si el
// servidor no esta en marcha, la mesa solo muestra la simulacion local de
// Rust, que es una demo y no el juego. Por eso al arrancar se comprueba el
// servidor y se intenta levantar si falta.

// ¿Esta escuchando el puerto RCON? Se parsea /proc/net/tcp porque no se
// presupone la disponibilidad de nc ni de netstat en la imagen de Android.
static bool rcon_port_listening() {
    const char* files[] = { "/proc/net/tcp", "/proc/net/tcp6" };
    char needle[8];
    snprintf(needle, sizeof(needle), ":%04X", RCON_PORT);

    for (int i = 0; i < 2; ++i) {
        FILE* fp = fopen(files[i], "r");
        if (!fp) continue;
        char line[512];
        while (fgets(line, sizeof(line), fp)) {
            // Columnas: sl local_address rem_address ...
            char local[64] = {0};
            if (sscanf(line, "%*d: %63s", local) != 1) continue;
            if (strstr(local, needle)) {
                fclose(fp);
                return true;
            }
        }
        fclose(fp);
    }
    return false;
}

// Localiza el linker de glibc. Android usa bionic y el motor ARM64 de
// Factorio exige glibc.
// Prepara el directorio de escritura del motor dentro del almacenamiento
// privado de la app: el motor no puede escribir en /data/local/tmp porque
// ese directorio pertenece al usuario "shell".
// Lanza el motor en segundo plano. Devuelve el PID o -1.
// Arranca el CLIENTE GRAFICO: es el que aporta el jugador que la campana
// exige (el nivel usa game.players[1] en su init) y el proceso donde se
// engancha el inyector del mod 3D.
//
// Va desde el sandbox de la app: alli SELinux si permite cargar Mesa, cosa
// que en /data/local/tmp no pasa ("Could not initialize OpenGL").
// Intenta arrancar el servidor con root (Magisk), que si puede ejecutar
// binarios alojados en /data/local/tmp.
// Comprueba el servidor y, si falta, intenta levantarlo. Cualquier fallo
// aqui no es fatal: la app arranca igualmente y el puente RCON se
// reconectara solo cuando el servidor aparezca.

// El motor de Factorio NO lo arranca la app.
//
// El proceso del motor hereda el contexto SELinux de la app (untrusted_app) y
// no puede leer /data/local/tmp (shell_data_file); ademas la Pico no deja
// ejecutar un ELF desde el sandbox de una app (execv -> Permission denied).
// El motor lo arranca el shell (scripts/deploy_pico.sh) y la app se limita a
// conectarse por RCON.
//
// Antes se intentaba copiar el motor al sandbox y arrancarlo desde ahi: el
// motor no llegaba ni a arrancar (execv -> Permission denied) y el RCON
// tampoco conectaba.
static int spawn_factorio_desde_la_app(const char* internal_data_path) {
    (void)internal_data_path;
    LOGI("Launcher: el motor lo arranca el shell; la app se conecta por RCON.");
    return -1;
}

static void ensure_factorio_server(ANativeActivity* activity) {
    if (rcon_port_listening()) {
        LOGI("Servidor Factorio detectado en 127.0.0.1:%d", RCON_PORT);
        return;
    }

    LOGI("No hay servidor Factorio en marcha; se intenta arrancar en modo standalone...");

    // La app no puede arrancar el motor (ver spawn_factorio_desde_la_app):
    // lo lanza el shell con scripts/deploy_pico.sh.
    LOGI("El motor lo arranca el shell: sh /data/local/tmp/start_factorio_standalone.sh");

    // Dar tiempo a que el motor abra el puerto antes de que la app intente
    // conectarse por RCON.
    for (int i = 0; i < 20; ++i) {
        usleep(500000);
        if (rcon_port_listening()) {
            LOGI("Servidor Factorio listo en 127.0.0.1:%d", RCON_PORT);
            return;
        }
    }
    LOGE("El servidor Factorio no respondio. Se ejecutara la simulacion local.");
    LOGE("Para arrancarlo a mano: sh %s", START_SCRIPT);
}

// El cliente grafico aporta el jugador que la campana exige y el proceso
// donde se engancha el inyector del mod 3D.  Se arranca despues de que el
// servidor este escuchando.
static void ensure_factorio_client(ANativeActivity* activity) {
    static bool ya_lanzado = false;
    if (ya_lanzado) return;
    ya_lanzado = true;

    pid_t pid = spawn_factorio_desde_la_app(activity->internalDataPath);
    if (pid > 0) {
        LOGI("Cliente grafico lanzado con PID %d", (int)pid);
    } else {
        LOGE("No se pudo lanzar el cliente grafico (el motor esta en el sandbox).");
    }
}

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

    // Arrancar el motor real de Factorio dentro del visor. Sin esto la mesa
    // solo refleja la simulacion local de Rust (modo demo).
    ensure_factorio_server(activity);
    ensure_factorio_client(activity);

    // Arrancar el motor y sesión estéreo OpenXR para Pico OS
    start_openxr_thread(activity);

    LOGI("Entorno listo. Factorio headless y el Tabletop OpenXR están activos.");
}
