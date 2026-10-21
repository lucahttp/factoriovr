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
static const char* FACTORIO_DIR = "/data/local/tmp/factorio_arm64";
static const char* FACTORIO_BIN = "/data/local/tmp/factorio_arm64/bin/arm64/factorio";
static const char* FACTORIO_MODS = "/data/local/tmp/factorio_arm64/mods";
static const char* START_SCRIPT = "/data/local/tmp/start_factorio_standalone.sh";
// Escenario de campana (el nivel de tutorial parcheado para multijugador).
// Lo deja el despliegue en el data del motor; si esta, el servidor arranca
// con el y no con una partida guardada.
static const char* CAMPAIGN_SCENARIO = "base/xfv-nivel";
static const char* CAMPAIGN_PATH =
    "/data/local/tmp/factorio_arm64/data/base/scenarios/xfv-nivel";
static const int   RCON_PORT = 27015;
static const char* RCON_PASSWORD = "xfactorvr";

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
static const char* find_glibc_loader() {
    static const char* candidates[] = {
        "/data/local/tmp/glibc/lib/ld-linux-aarch64.so.1",
        "/data/local/tmp/glibc/lib64/ld-linux-aarch64.so.1",
        "/data/local/tmp/factorio_arm64/glibc/lib/ld-linux-aarch64.so.1",
        "/data/data/com.termux/files/usr/glibc/lib/ld-linux-aarch64.so.1",
        "/data/data/com.termux/files/usr/lib/ld-linux-aarch64.so.1",
    };
    for (size_t i = 0; i < sizeof(candidates) / sizeof(candidates[0]); ++i) {
        if (access(candidates[i], X_OK) == 0) return candidates[i];
    }
    return nullptr;
}

// Escribe un fichero con permisos 0644 creando los directorios intermedios.
static bool write_file(const char* path, const char* content) {
    char dir[512];
    snprintf(dir, sizeof(dir), "%s", path);
    char* slash = strrchr(dir, '/');
    if (slash) {
        *slash = '\0';
        // Crear cada nivel (mkdir -p).
        for (char* p = dir + 1; *p; ++p) {
            if (*p == '/') { *p = '\0'; mkdir(dir, 0755); *p = '/'; }
        }
        mkdir(dir, 0755);
    }
    FILE* fp = fopen(path, "w");
    if (!fp) return false;
    fputs(content, fp);
    fclose(fp);
    chmod(path, 0644);
    return true;
}

// Prepara el directorio de escritura del motor dentro del almacenamiento
// privado de la app: el motor no puede escribir en /data/local/tmp porque
// ese directorio pertenece al usuario "shell".
static bool prepare_app_factorio_home(const char* internal_data_path) {
    char home[512], cfg_dir[512], cfg_path[512], ini_path[512], content[1400];
    snprintf(home, sizeof(home), "%s/factorio", internal_data_path);
    snprintf(cfg_dir, sizeof(cfg_dir), "%s/config", home);
    snprintf(cfg_path, sizeof(cfg_path), "%s/config-path.cfg", home);
    snprintf(ini_path, sizeof(ini_path), "%s/config/config.ini", home);

    mkdir(home, 0755);
    mkdir(cfg_dir, 0755);
    mkdir("/data/local/tmp/factorio_arm64/saves", 0755);

    snprintf(content, sizeof(content),
             "config-path=%s/config\n"
             "use-system-read-write-data-directories=false\n", home);
    if (!write_file(cfg_path, content)) return false;

    snprintf(content, sizeof(content),
             "[path]\n"
             "read-data=%s/data\n"
             "write-data=%s\n", FACTORIO_DIR, home);
    if (!write_file(ini_path, content)) return false;

    // Ajustes del servidor.  El motor los lee del directorio de escritura
    // (el de la app), asi que hay que ponerlos aqui y no solo en el motor.
    //
    // auto_pause=true congela el mapa mientras no haya nadie conectado.  Con
    // la campana es lo que queremos (el nivel espera al jugador); sin ella,
    // el mundo se quedaria parado en el visor.
    bool campana = (access(CAMPAIGN_PATH, F_OK) == 0);
    snprintf(content, sizeof(content),
             "{\n"
             "  \"name\": \"XFactorVR\",\n"
             "  \"description\": \"Factorio para Realidad Mixta\",\n"
             "  \"visibility\": { \"public\": false, \"lan\": true },\n"
             "  \"require_user_verification\": false,\n"
             "  \"auto_pause\": %s,\n"
             "  \"auto_pause_when_players_connect\": false,\n"
             "  \"autosave_interval\": 10,\n"
             "  \"autosave_slots\": 3\n"
             "}\n", campana ? "true" : "false");
    snprintf(cfg_dir, sizeof(cfg_dir), "%s/config/server-settings.json", home);
    write_file(cfg_dir, content);
    return true;
}

// Lanza el motor en segundo plano. Devuelve el PID o -1.
static pid_t spawn_factorio_server(const char* internal_data_path) {
    const char* loader = find_glibc_loader();
    if (!loader) {
        LOGE("No se encuentra el linker de glibc. El motor de Factorio no puede arrancar.");
        return -1;
    }

    char lib_dir[512];
    snprintf(lib_dir, sizeof(lib_dir), "%s", loader);
    char* slash = strrchr(lib_dir, '/');
    if (slash) *slash = '\0';

    char home[512], cfg_path[512], save[600], mod_dir[512], saves_dir[600];
    snprintf(home, sizeof(home), "%s/factorio", internal_data_path);
    snprintf(cfg_path, sizeof(cfg_path), "%s/config-path.cfg", home);
    snprintf(saves_dir, sizeof(saves_dir), "%s/saves", home);
    snprintf(save, sizeof(save), "%s/saves/xfactorvr_world.zip", home);
    snprintf(mod_dir, sizeof(mod_dir), "%s", FACTORIO_MODS);
    mkdir(saves_dir, 0755);

    // Si el despliegue dejo el escenario de campana, el servidor arranca con
    // el (el nivel de campana es single-player y necesita un jugador real).
    bool campana = (access(CAMPAIGN_PATH, F_OK) == 0);

    if (!prepare_app_factorio_home(internal_data_path)) {
        LOGE("No se pudo preparar el directorio privado del motor: %s", home);
        return -1;
    }

    // La partida se crea la primera vez; si ya existe se reutiliza.
    if (!campana && access(save, F_OK) != 0) {
        LOGI("Creando la partida inicial del motor...");
        pid_t cpid = fork();
        if (cpid == 0) {
            setenv("LD_LIBRARY_PATH", lib_dir, 1);
            char* args[] = {
                (char*)loader, (char*)"--library-path", lib_dir, (char*)FACTORIO_BIN,
                // Ruta ABSOLUTA: con un nombre relativo el motor la crea en
                // el directorio de trabajo y luego no aparece en saves/.
                (char*)"--create", (char*)save,
                (char*)"--mod-directory", mod_dir,
                (char*)"--config", cfg_path,
                nullptr
            };
            execv(loader, args);
            _exit(127);
        }
        if (cpid > 0) {
            int status = 0;
            waitpid(cpid, &status, 0);
        }
    }

    LOGI("Arrancando el motor Factorio headless (RCON en 127.0.0.1:%d)...", RCON_PORT);
    pid_t pid = fork();
    if (pid == 0) {
        // El motor hereda este proceso: hay que desprenderlo para que no
        // muera cuando la app se cierre.
        setsid();
        setenv("LD_LIBRARY_PATH", lib_dir, 1);
        setenv("HOME", home, 1);
        char* args_campana[] = {
            (char*)loader, (char*)"--library-path", lib_dir, (char*)FACTORIO_BIN,
            (char*)"--start-server-load-scenario", (char*)CAMPAIGN_SCENARIO,
            (char*)"--rcon-port", (char*)"27015",
            (char*)"--rcon-password", (char*)RCON_PASSWORD,
            (char*)"--mod-directory", mod_dir,
            (char*)"--config", cfg_path,
            nullptr
        };
        char* args_partida[] = {
            (char*)loader, (char*)"--library-path", lib_dir, (char*)FACTORIO_BIN,
            (char*)"--start-server", save,
            (char*)"--rcon-port", (char*)"27015",
            (char*)"--rcon-password", (char*)RCON_PASSWORD,
            (char*)"--mod-directory", mod_dir,
            (char*)"--config", cfg_path,
            nullptr
        };
        execv(loader, campana ? args_campana : args_partida);
        _exit(127);
    }
    return pid;
}

// Intenta arrancar el servidor con root (Magisk), que si puede ejecutar
// binarios alojados en /data/local/tmp.
static bool spawn_via_su() {
    if (access("/system/bin/su", X_OK) != 0 && access("/system/xbin/su", X_OK) != 0) {
        return false;
    }
    pid_t pid = fork();
    if (pid == 0) {
        setsid();
        execl("/system/bin/su", "su", "-c",
              "sh /data/local/tmp/start_factorio_standalone.sh >/dev/null 2>&1",
              (char*)nullptr);
        _exit(127);
    }
    return true;
}

// Comprueba el servidor y, si falta, intenta levantarlo. Cualquier fallo
// aqui no es fatal: la app arranca igualmente y el puente RCON se
// reconectara solo cuando el servidor aparezca.
static void ensure_factorio_server(ANativeActivity* activity) {
    if (rcon_port_listening()) {
        LOGI("Servidor Factorio detectado en 127.0.0.1:%d", RCON_PORT);
        return;
    }

    LOGI("No hay servidor Factorio en marcha; se intenta arrancar en modo standalone...");

    if (spawn_via_su()) {
        LOGI("Lanzamiento del servidor solicitado via su (root).");
    } else {
        pid_t pid = spawn_factorio_server(activity->internalDataPath);
        if (pid > 0) {
            LOGI("Motor lanzado con PID %d", (int)pid);
        }
    }

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

    // Arrancar el motor y sesión estéreo OpenXR para Pico OS
    start_openxr_thread(activity);

    LOGI("Entorno listo. Factorio headless y el Tabletop OpenXR están activos.");
}
