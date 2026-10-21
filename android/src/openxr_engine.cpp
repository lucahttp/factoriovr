// openxr_engine.cpp — Motor OpenXR Standalone para Pico 4 Ultra (Tabletop Passthrough)
// Arquitectura completa inspirada en Generals-Zero-Hour-XR + Factorio Linux ARM64 + factorio-3d-mod
#include "openxr_engine.h"

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <GLES3/gl3ext.h>

#define XR_USE_PLATFORM_ANDROID 1
#define XR_USE_GRAPHICS_API_OPENGL_ES 1
#include <openxr/openxr.h>
#include <openxr/openxr_platform.h>
#include <android/log.h>
#include <pthread.h>
#include <unistd.h>
#include <vector>
#include <string>
#include <cmath>
#include <cstring>
#include <algorithm>
#include <chrono>
#include <dlfcn.h>
#include <aaudio/AAudio.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <cerrno>
#include <mutex>
#include <atomic>

#define TAG "XFactorVR_OpenXR"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, TAG, __VA_ARGS__)

static pthread_t s_engine_thread;
static volatile bool s_running = false;
static ANativeActivity* s_activity = nullptr;

// ============================================================================
// AUDIO ESPACIAL 3D POSICIONAL (XR AUDIO — GENERALS ZERO HOUR XR PATTERN)
// ============================================================================
enum AudioSfxType {
    SFX_PLACE = 0,
    SFX_MINE = 1,
    SFX_CRAFT = 2,
    SFX_CLICK = 3
};

struct ActiveSfx {
    AudioSfxType type;
    float time;
    float duration;
    float pan;
    float volume;
};

static std::vector<ActiveSfx> s_active_sfx;
static pthread_mutex_t s_audio_mutex = PTHREAD_MUTEX_INITIALIZER;
static AAudioStream* s_audio_stream = nullptr;
static XrVector3f s_listener_pos = { 0.0f, 0.0f, 0.0f };
static float s_listener_yaw = 0.0f;
static float s_factory_activity_level = 0.2f;

static aaudio_data_callback_result_t audio_data_callback(
    AAudioStream* stream,
    void* userData,
    void* audioData,
    int32_t numFrames) {
    
    float* buffer = (float*)audioData;
    static float s_phase_hum = 0.0f;

    pthread_mutex_lock(&s_audio_mutex);

    for (int32_t i = 0; i < numFrames; ++i) {
        float left = 0.0f;
        float right = 0.0f;

        // 1. Zumbido continuo de maquinaria industrial (55 Hz con armónico a 110 Hz)
        if (s_factory_activity_level > 0.01f) {
            s_phase_hum += (2.0f * 3.14159265f * 55.0f) / 48000.0f;
            if (s_phase_hum > 2.0f * 3.14159265f) s_phase_hum -= 2.0f * 3.14159265f;
            float hum = (sinf(s_phase_hum) * 0.020f + sinf(s_phase_hum * 2.0f) * 0.010f) * s_factory_activity_level;
            left += hum;
            right += hum;
        }

        // 2. Efectos sonoros espaciales 3D
        for (size_t s = 0; s < s_active_sfx.size(); ++s) {
            ActiveSfx& sfx = s_active_sfx[s];
            float t = sfx.time;
            float sample = 0.0f;

            if (sfx.type == SFX_PLACE) {
                // Golpe metálico sordo y sólido al construir
                float freq = 160.0f - (t / sfx.duration) * 110.0f;
                float env = expf(-t * 14.0f);
                sample = sinf(2.0f * 3.14159265f * freq * t) * env * 0.35f;
            } else if (sfx.type == SFX_MINE) {
                // Carraca metálica de desmontaje
                float env = expf(-t * 10.0f);
                float mod = (sinf(2.0f * 3.14159265f * 28.0f * t) > 0.0f) ? 1.0f : 0.2f;
                sample = sinf(2.0f * 3.14159265f * 480.0f * t) * mod * env * 0.28f;
            } else if (sfx.type == SFX_CRAFT) {
                // Campanilla yunque brillante al fabricar ítem
                float env = expf(-t * 6.0f);
                sample = (sinf(2.0f * 3.14159265f * 1320.0f * t) * 0.7f +
                          sinf(2.0f * 3.14159265f * 2640.0f * t) * 0.3f) * env * 0.30f;
            } else if (sfx.type == SFX_CLICK) {
                // Clic acústico de botón UI
                float env = expf(-t * 90.0f);
                sample = sinf(2.0f * 3.14159265f * 980.0f * t) * env * 0.20f;
            }

            float pan_norm = std::clamp(sfx.pan, -1.0f, 1.0f);
            float angle = (pan_norm + 1.0f) * 0.25f * 3.14159265f;
            float gl = cosf(angle);
            float gr = sinf(angle);

            left += sample * gl * sfx.volume;
            right += sample * gr * sfx.volume;

            sfx.time += 1.0f / 48000.0f;
        }

        buffer[i * 2]     = std::clamp(left, -1.0f, 1.0f);
        buffer[i * 2 + 1] = std::clamp(right, -1.0f, 1.0f);
    }

    for (size_t s = 0; s < s_active_sfx.size(); ) {
        if (s_active_sfx[s].time >= s_active_sfx[s].duration) {
            s_active_sfx[s] = s_active_sfx.back();
            s_active_sfx.pop_back();
        } else {
            ++s;
        }
    }

    pthread_mutex_unlock(&s_audio_mutex);
    return AAUDIO_CALLBACK_RESULT_CONTINUE;
}

static void init_audio_system() {
    AAudioStreamBuilder* builder = nullptr;
    if (AAudio_createStreamBuilder(&builder) != AAUDIO_OK) {
        LOGE("Error creando AAudioStreamBuilder");
        return;
    }
    AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_FLOAT);
    AAudioStreamBuilder_setChannelCount(builder, 2);
    AAudioStreamBuilder_setSampleRate(builder, 48000);
    AAudioStreamBuilder_setDataCallback(builder, audio_data_callback, nullptr);
    AAudioStreamBuilder_setPerformanceMode(builder, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);

    aaudio_result_t res = AAudioStreamBuilder_openStream(builder, &s_audio_stream);
    AAudioStreamBuilder_delete(builder);

    if (res == AAUDIO_OK && s_audio_stream) {
        AAudioStream_requestStart(s_audio_stream);
        LOGI("Motor de Audio Espacial 3D (AAudio) inicializado a 48000Hz stereo");
    } else {
        LOGE("No se pudo iniciar el stream de AAudio: %d", res);
        s_audio_stream = nullptr;
    }
}

static void shutdown_audio_system() {
    if (s_audio_stream) {
        AAudioStream_requestStop(s_audio_stream);
        AAudioStream_close(s_audio_stream);
        s_audio_stream = nullptr;
    }
}

static void play_spatial_sound(AudioSfxType type, float wx, float wy, float wz, float vol = 1.0f) {
    float dx = wx - s_listener_pos.x;
    float dz = wz - s_listener_pos.z;
    float dist = sqrtf(dx * dx + dz * dz);
    float dist_att = 1.0f / (1.0f + dist * 1.6f);

    float angle_to_sound = atan2f(dx, -dz);
    float rel_angle = angle_to_sound - s_listener_yaw;
    float pan = sinf(rel_angle);

    float duration = 0.25f;
    if (type == SFX_CRAFT) duration = 0.50f;
    else if (type == SFX_PLACE) duration = 0.32f;
    else if (type == SFX_CLICK) duration = 0.04f;

    pthread_mutex_lock(&s_audio_mutex);
    if (s_active_sfx.size() < 16) {
        s_active_sfx.push_back({type, 0.0f, duration, pan, vol * dist_att});
    }
    pthread_mutex_unlock(&s_audio_mutex);
}

// ============================================================================
// MATEMÁTICAS 3D Y TRANSFORMACIONES (OPENGL ES COLUMN-MAJOR)
// ============================================================================

// ============================================================================
// MATEMÁTICAS 3D Y TRANSFORMACIONES (BATTLE-TESTED GENERALS XR COLUMN-MAJOR)
// ============================================================================

static void matPerspectiveFromFov(float* m, const XrFovf& fov, float n, float f) {
    const float l = tanf(fov.angleLeft);
    const float r = tanf(fov.angleRight);
    const float u = tanf(fov.angleUp);
    const float d = tanf(fov.angleDown);
    memset(m, 0, 16 * sizeof(float));
    m[0] = 2.0f / (r - l);
    m[8] = (r + l) / (r - l);
    m[5] = 2.0f / (u - d);
    m[9] = (u + d) / (u - d);
    m[10] = -(f + n) / (f - n);
    m[11] = -1.0f;
    m[14] = -2.0f * f * n / (f - n);
}

static void matViewFromPose(float* m, const XrPosef& p) {
    const float x = p.orientation.x, y = p.orientation.y;
    const float z = p.orientation.z, w = p.orientation.w;
    const float xx = x * x, yy = y * y, zz = z * z;
    const float xy = x * y, xz = x * z, yz = y * z;
    const float wx = w * x, wy = w * y, wz = w * z;

    const float r00 = 1.0f - 2.0f * (yy + zz);
    const float r01 = 2.0f * (xy - wz);
    const float r02 = 2.0f * (xz + wy);
    const float r10 = 2.0f * (xy + wz);
    const float r11 = 1.0f - 2.0f * (xx + zz);
    const float r12 = 2.0f * (yz - wx);
    const float r20 = 2.0f * (xz - wy);
    const float r21 = 2.0f * (yz + wx);
    const float r22 = 1.0f - 2.0f * (xx + yy);

    m[0] = r00; m[4] = r10; m[8] = r20; m[12] = 0.0f;
    m[1] = r01; m[5] = r11; m[9] = r21; m[13] = 0.0f;
    m[2] = r02; m[6] = r12; m[10] = r22; m[14] = 0.0f;
    m[3] = 0.0f; m[7] = 0.0f; m[11] = 0.0f; m[15] = 1.0f;

    const float tx = p.position.x, ty = p.position.y, tz = p.position.z;
    m[12] = -(m[0] * tx + m[4] * ty + m[8] * tz);
    m[13] = -(m[1] * tx + m[5] * ty + m[9] * tz);
    m[14] = -(m[2] * tx + m[6] * ty + m[10] * tz);
}

static void matMultiply(float* out, const float* a, const float* b) {
    float tmp[16];
    for (int c = 0; c < 4; c++) {
        for (int r = 0; r < 4; r++) {
            tmp[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] +
                             a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
        }
    }
    memcpy(out, tmp, sizeof(tmp));
}

static void matIdentity(float* m) {
    memset(m, 0, 16 * sizeof(float));
    m[0] = m[5] = m[10] = m[15] = 1.0f;
}

static void matTranslate(float* m, float x, float y, float z) {
    matIdentity(m);
    m[12] = x;
    m[13] = y;
    m[14] = z;
}

static void matRotY(float* m, float rad) {
    const float c = cosf(rad), s = sinf(rad);
    memset(m, 0, 16 * sizeof(float));
    m[0] = c;  m[8] = s;
    m[5] = 1.0f;
    m[2] = -s; m[10] = c;
    m[15] = 1.0f;
}

static void matScale(float* m, float sx, float sy, float sz) {
    matIdentity(m);
    m[0] = sx;
    m[5] = sy;
    m[10] = sz;
}

static void yawForwardFromQuat(const XrQuaternionf& q, float* fx, float* fz) {
    const float x = q.x, y = q.y, z = q.z, w = q.w;
    const float r02 = 2.0f * (x * z + w * y);
    const float r22 = 1.0f - 2.0f * (x * x + y * y);
    float ux = -r02, uz = -r22;
    const float len = sqrtf(ux * ux + uz * uz);
    if (len < 1e-4f) {
        *fx = 0.0f;
        *fz = -1.0f;
        return;
    }
    *fx = ux / len;
    *fz = uz / len;
}

static bool selfCheckMath() {
    const XrPosef identity = {{0.0f, 0.0f, 0.0f, 1.0f}, {0.0f, 0.0f, 0.0f}};
    float m[16];
    matViewFromPose(m, identity);
    for (int i = 0; i < 16; i++) {
        const float want = (i % 5 == 0) ? 1.0f : 0.0f;
        if (fabsf(m[i] - want) > 1e-5f) {
            LOGE("math self-check FAILED: identity view m[%d]=%f", i, m[i]);
            return false;
        }
    }
    float fx = 0.0f, fz = 0.0f;
    yawForwardFromQuat(identity.orientation, &fx, &fz);
    if (fabsf(fx) > 1e-5f || fabsf(fz + 1.0f) > 1e-5f) {
        LOGE("math self-check FAILED: identity yaw-forward=(%f,%f)", fx, fz);
        return false;
    }
    LOGI("math self-check PASS: Generals XR matrix math validado correctamente");
    return true;
}

// Wrappers con el orden de argumentos del motor
static inline void mat4_perspective_openxr(const XrFovf& fov, float nearZ, float farZ, float* m) {
    matPerspectiveFromFov(m, fov, nearZ, farZ);
}

static inline void mat4_view_from_pose(const XrPosef& pose, float* m) {
    matViewFromPose(m, pose);
}

static inline void mat4_multiply(const float* a, const float* b, float* out) {
    matMultiply(out, a, b);
}

[[maybe_unused]] static inline void mat4_identity(float* m) {
    matIdentity(m);
}

static inline void mat4_translate(float* m, float x, float y, float z) {
    matTranslate(m, x, y, z);
}

static inline void mat4_rotate_y(float* m, float rad) {
    matRotY(m, rad);
}

static inline void mat4_scale(float* m, float sx, float sy, float sz) {
    matScale(m, sx, sy, sz);
}

// ============================================================================
// ESTRUCTURAS OPENXR Y OPENGL
// ============================================================================

struct EyeSwapchain {
    XrSwapchain handle = XR_NULL_HANDLE;
    int32_t width = 0;
    int32_t height = 0;
    std::vector<XrSwapchainImageOpenGLESKHR> images;
    std::vector<GLuint> fbos;
    std::vector<GLuint> depth_rbs;
};

// Shaders con soporte para texturas, UV offset dinámico, recorte de mesa y partes emisivas
static const char* VERTEX_SHADER =
    "#version 300 es\n"
    "layout(location = 0) in vec3 a_pos;\n"
    "layout(location = 1) in vec3 a_normal;\n"
    "layout(location = 2) in vec2 a_uv;\n"
    "layout(location = 3) in vec4 a_color;\n"
    "uniform mat4 u_mvp;\n"
    "uniform mat4 u_model;\n"
    "uniform vec2 u_uv_offset;\n"
    "uniform float u_lit;\n"
    "out vec4 v_color;\n"
    "out vec3 v_world_pos;\n"
    "out vec3 v_local_pos;\n"
    "out vec3 v_normal;\n"
    "out vec2 v_uv;\n"
    "void main() {\n"
    "    v_local_pos = a_pos;\n"
    "    vec4 world_p = u_model * vec4(a_pos, 1.0);\n"
    "    v_world_pos = world_p.xyz;\n"
    "    v_normal = normalize((u_model * vec4(a_normal, 0.0)).xyz);\n"
    "    v_uv = a_uv + u_uv_offset;\n"
    "    v_color = a_color;\n"
    "    gl_Position = u_mvp * vec4(a_pos, 1.0);\n"
    "}\n";

static const char* FRAGMENT_SHADER =
    "#version 300 es\n"
    "precision mediump float;\n"
    "in vec4 v_color;\n"
    "in vec3 v_world_pos;\n"
    "in vec3 v_local_pos;\n"
    "in vec3 v_normal;\n"
    "in vec2 v_uv;\n"
    "uniform vec4 u_board_bounds;\n" // min_x, min_z, max_x, max_z en coordenadas locales de mesa
    "uniform float u_enable_clipping;\n" // 1.0 = recortar límites de la mesa física
    "uniform float u_lit;\n"
    "uniform vec3 u_emissive;\n" // Brillo propio que no se ve afectado por la luz solar
    "uniform sampler2D u_tex;\n"
    "uniform float u_use_tex;\n"
    "out vec4 fragColor;\n"
    "void main() {\n"
    "    if (u_enable_clipping > 0.5) {\n"
    "        if (v_local_pos.x < u_board_bounds.x || v_local_pos.x > u_board_bounds.z ||\n"
    "            v_local_pos.z < u_board_bounds.y || v_local_pos.z > u_board_bounds.w) {\n"
    "            discard;\n"
    "        }\n"
    "    }\n"
    "    vec4 base_col = v_color;\n"
    "    if (u_use_tex > 0.5) {\n"
    "        vec4 tex_sample = texture(u_tex, v_uv);\n"
    "        base_col = tex_sample * v_color;\n"
    "    }\n"
    "    if (u_lit > 0.5) {\n"
    "        vec3 light_dir = normalize(vec3(0.35, 0.90, 0.30));\n"
    "        vec3 n = normalize(v_normal);\n"
    "        float diff = max(dot(n, light_dir), 0.0);\n"
    "        float amb = 0.48;\n"
    "        vec3 view_dir = normalize(-v_world_pos);\n"
    "        vec3 half_v = normalize(light_dir + view_dir);\n"
    "        float spec = pow(max(dot(n, half_v), 0.0), 16.0) * 0.24;\n"
    "        vec3 lit_rgb = base_col.rgb * (amb + (1.0 - amb) * diff) + vec3(spec) + u_emissive;\n"
    "        fragColor = vec4(lit_rgb, base_col.a);\n"
    "    } else {\n"
    "        fragColor = vec4(base_col.rgb + u_emissive, base_col.a);\n"
    "    }\n"
    "}\n";

static GLuint s_program = 0;
static GLint s_mvp_loc = -1;
static GLint s_model_loc = -1;
static GLint s_uv_offset_loc = -1;
static GLint s_lit_loc = -1;
static GLint s_emissive_loc = -1;
static GLint s_board_bounds_loc = -1;
static GLint s_enable_clipping_loc = -1;
static GLint s_tex_loc = -1;
static GLint s_use_tex_loc = -1;

struct DioramaVertex {
    float x, y, z;
    float nx, ny, nz;
    float u, v;
    float r, g, b, a;
};

// Dimensiones de la mesa de diorama (en metros)
static const float TABLE_HALF_W = 0.60f;  // Ancho total 1.20m
static const float TABLE_HALF_D = 0.40f;  // Profundidad total 0.80m

// ============================================================================
// ESTADO DE MESA Y OBSERVADOR HUMANO (GENERALS XR PATTERN)
// ============================================================================

static bool s_table_placed = false;
static XrVector3f s_table_pos = { 0.0f, 0.74f, -1.05f };
static float s_table_yaw = 0.0f;
static float s_table_scale = 1.0f;
static float s_table_model_mat[16];

// Modo Observador a escala humana 1:1 (XrWorld::observer)
static bool s_observer_mode = false;
static XrVector3f s_observer_pos = { 0.0f, 0.0f, 0.0f }; // Posición caminable sobre el suelo de la fábrica

// Manipulación de mesa (XrSurfaceGrab)
static bool s_table_grabbed = false;
static XrVector3f s_grab_start_hand = { 0.0f, 0.0f, 0.0f };
static XrVector3f s_grab_start_table = { 0.0f, 0.0f, 0.0f };

// Sub-panel activo de la consola UI holográfica (0 = Construcción, 1 = Inventario, 2 = Fabricar)
static int s_active_ui_tab = 0;
static int s_hovered_ui_btn = -1;
static int s_hovered_recipe = -1;

// Recetas disponibles en la pestaña Fabricar.
#define CRAFT_RECIPE_COUNT 7

// Extensiones OpenXR negociadas con el runtime PICO 3.0.0.
// Se consultan con xrEnumerateInstanceExtensionProperties y solo se habilitan
// las realmente soportadas: activar una extensión ausente aborta xrCreateInstance.
static bool s_has_fb_passthrough   = false; // XR_FB_passthrough
static bool s_has_bd_controller    = false; // XR_BD_controller_interaction
static bool s_has_ext_perf         = false; // XR_EXT_performance_settings
static bool s_has_fb_refresh       = false; // XR_FB_display_refresh_rate

// Texturas Factorio procedurales
static GLuint s_tex_concrete = 0;
static GLuint s_tex_hazard   = 0;
static GLuint s_tex_belt     = 0;
static GLuint s_tex_soil     = 0;
static GLuint s_tex_steel    = 0;

// ============================================================================
// SIMULACIÓN EN VIVO FACTORIO, ENTIDADES Y MODELOS 3D (FASE 1, 2, 3)
// ============================================================================

struct LiveEntity {
    uint32_t entity_type;
    float tile_x;
    float tile_y;
    float orientation;
    uint32_t status;
    float animation_progress;
    float health_ratio;
    float arm_angle;   // angulo del brazo en radianes (solo insertadores)
    uint32_t sprite_id;  // sprite real de Factorio en el atlas (0 = sin sprite)
    uint32_t anim_frame;  // frame de animacion actual
};

// Tabla de tipos de entidad. Debe coincidir con src/tabletop.rs y con el mod
// puente mods/xfactorvr_bridge/control.lua.
enum EntityType {
    ENTITY_NONE = 0,
    ENTITY_ASSEMBLING_MACHINE = 1,
    ENTITY_MINING_DRILL = 2,
    ENTITY_FURNACE = 3,
    ENTITY_GUN_TURRET = 4,
    ENTITY_RADAR = 5,
    ENTITY_ROBOPORT = 6,
    ENTITY_CHEST = 7,
    ENTITY_ELECTRIC_POLE = 8,
    ENTITY_TRANSPORT_BELT = 9,
    ENTITY_INSERTER = 10,
    ENTITY_BITER = 11,
    ENTITY_TREE = 12,
    ENTITY_PLAYER = 13,
    ENTITY_TYPE_COUNT = 14
};

struct PlayerItemFfi {
    uint32_t item_id;
    uint32_t count;
};

struct ResourcePatchFfi {
    uint32_t item_type;
    float tile_x;
    float tile_y;
    float radius;
    uint32_t remaining;
};

typedef bool (*pfn_xfactorvr_load_glb)(const char* path, float target_width, DioramaVertex** out_verts, int* out_count);
typedef void (*pfn_xfactorvr_free_glb)(DioramaVertex* verts, int count);
typedef bool (*pfn_xfactorvr_get_live_entities)(LiveEntity* out_entities, int max_count, int* out_count);
typedef bool (*pfn_xfactorvr_place_or_update_entity)(uint32_t entity_type, float tile_x, float tile_y, float orientation);
typedef bool (*pfn_xfactorvr_mine_entity)(float tile_x, float tile_y);
typedef void (*pfn_xfactorvr_update_simulation)(float delta_time);

// --- Atlas de sprites reales de Factorio -------------------------------------
// El renderer deja de inventar primitivas: dibuja los sprites que usa el juego,
// recortados del atlas que extrae scripts/extract_factorio_assets.py.
typedef int  (*pfn_xfactorvr_get_atlas_page_count)();
typedef bool (*pfn_xfactorvr_get_atlas_page_size)(uint32_t page, int* out_w, int* out_h);
typedef int  (*pfn_xfactorvr_get_atlas_page_pixels)(uint32_t page, uint8_t* out, int max_len);
typedef bool (*pfn_xfactorvr_get_sprite_frame)(uint32_t sprite_id, uint32_t anim_frame,
                                               float orientation, float* out);

static pfn_xfactorvr_get_atlas_page_count   s_get_atlas_page_count   = nullptr;
static pfn_xfactorvr_get_atlas_page_size    s_get_atlas_page_size    = nullptr;
static pfn_xfactorvr_get_atlas_page_pixels  s_get_atlas_page_pixels  = nullptr;
static pfn_xfactorvr_get_sprite_frame       s_get_sprite_frame       = nullptr;

// --- Atlas del suelo --------------------------------------------------------
// El terreno real del mapa (hierba, tierra, arena...) viene en celdas de
// 32x32 recortadas de las hojas del juego y empaquetadas en un atlas
// paginado, igual que los sprites.  Android no decodifica PNG, asi que el
// Rust lo entrega en crudo.

// El atlas se parte en paginas: una textura de mas de 4096 px de alto no se
// puede subir en la GPU movil (GL_MAX_TEXTURE_SIZE).
#define XFACTORVR_MAX_ATLAS_PAGES 32

typedef int  (*pfn_xfactorvr_get_terrain_page_count)();
typedef bool (*pfn_xfactorvr_get_terrain_page_size)(uint32_t page, int* out_w, int* out_h);
typedef int  (*pfn_xfactorvr_get_terrain_page_pixels)(uint32_t page, uint8_t* out, int max_len);
typedef int  (*pfn_xfactorvr_get_terrain_tile_size)();
typedef int  (*pfn_xfactorvr_get_terrain_variant_count)(const char* nombre);
typedef bool (*pfn_xfactorvr_get_terrain_tile)(const char* nombre, uint32_t variant, float* out);
typedef int  (*pfn_xfactorvr_get_terrain_mesh)(float* out, int max_verts);

static pfn_xfactorvr_get_terrain_page_count    s_get_terrain_page_count    = nullptr;
static pfn_xfactorvr_get_terrain_page_size     s_get_terrain_page_size     = nullptr;
static pfn_xfactorvr_get_terrain_page_pixels   s_get_terrain_page_pixels   = nullptr;
static pfn_xfactorvr_get_terrain_tile_size     s_get_terrain_tile_size     = nullptr;
static pfn_xfactorvr_get_terrain_variant_count s_get_terrain_variant_count = nullptr;
static pfn_xfactorvr_get_terrain_tile          s_get_terrain_tile          = nullptr;
static pfn_xfactorvr_get_terrain_mesh           s_get_terrain_mesh           = nullptr;

static GLuint s_tex_terrain[XFACTORVR_MAX_ATLAS_PAGES] = {0};
static int    s_terrain_page_w[XFACTORVR_MAX_ATLAS_PAGES] = {0};
static int    s_terrain_page_h[XFACTORVR_MAX_ATLAS_PAGES] = {0};
static int    s_terrain_page_count = 0;
static bool   s_terrain_probed = false;
// Contador de fotogramas: la malla del suelo se rehace cada 30 frames.
static int    s_terrain_frame = 0;

// El atlas se parte en paginas: una textura de mas de 4096 px de alto no se
// puede subir en la GPU movil (GL_MAX_TEXTURE_SIZE).
static GLuint s_tex_atlas[XFACTORVR_MAX_ATLAS_PAGES] = {0};
static int    s_atlas_page_w[XFACTORVR_MAX_ATLAS_PAGES] = {0};
static int    s_atlas_page_h[XFACTORVR_MAX_ATLAS_PAGES] = {0};
static int    s_atlas_page_count = 0;
static bool   s_atlas_probed = false;

struct SpriteBatch {
    GLuint vao = 0;
    GLuint vbo = 0;
    int vertex_count = 0;
    std::vector<DioramaVertex> verts;
};
static SpriteBatch s_sprite_batches[XFACTORVR_MAX_ATLAS_PAGES];
// El suelo va en sus propios lotes (una por pagina del atlas de terreno).
static SpriteBatch s_terrain_batches[XFACTORVR_MAX_ATLAS_PAGES];
typedef bool (*pfn_xfactorvr_get_player_inventory)(PlayerItemFfi* out_items, uint32_t max_items, uint32_t* out_count);
typedef bool (*pfn_xfactorvr_craft_item)(uint32_t item_id, uint32_t count);
typedef bool (*pfn_xfactorvr_get_resource_patches)(ResourcePatchFfi* out_patches, uint32_t max_patches, uint32_t* out_count);
typedef bool (*pfn_xfactorvr_connect_rcon)(const char* host, uint16_t port, const char* pwd);
typedef bool (*pfn_xfactorvr_is_rcon_connected)();
typedef uint32_t (*pfn_xfactorvr_manual_mine)(float tile_x, float tile_y);
typedef uint32_t (*pfn_xfactorvr_interact_machine)(float tile_x, float tile_y);
typedef bool (*pfn_xfactorvr_reset_world)(bool fresh_start);
typedef bool (*pfn_xfactorvr_get_machine_info)(float tile_x, float tile_y, float* out_fuel, float* out_progress, uint32_t* out_input_ore, uint32_t* out_output_plates);
typedef bool (*pfn_xfactorvr_get_carried_item)(float tile_x, float tile_y, uint32_t* out_item_id, uint32_t* out_count);
// El lado Rust devuelve bool en estos dos: el valor de retorno debe coincidir
// o el ABI queda desalineado entre la libreria y el engine.
typedef bool (*pfn_xfactorvr_pan_view)(float delta_x, float delta_y);
typedef bool (*pfn_xfactorvr_get_view_center)(float* out_x, float* out_y);
// Accion de las manos en VR: decide y ejecuta (coger del suelo, sacar de la
// caja, meter en el horno o en la caja).  `hand`: 0 = gatillo, 1 = gripo.
// Devuelve false si no habia nada que hacer (entonces el motor sigue con su
// comportamiento normal de raton).
typedef bool (*pfn_xfactorvr_vr_hand_action)(float tile_x, float tile_y, uint32_t hand);

static pfn_xfactorvr_load_glb s_load_glb = nullptr;
static pfn_xfactorvr_free_glb s_free_glb = nullptr;
static pfn_xfactorvr_get_live_entities s_get_live_entities = nullptr;
static pfn_xfactorvr_place_or_update_entity s_place_or_update_entity = nullptr;
static pfn_xfactorvr_mine_entity s_mine_entity = nullptr;
static pfn_xfactorvr_update_simulation s_update_simulation = nullptr;
static pfn_xfactorvr_get_player_inventory s_get_player_inventory = nullptr;
static pfn_xfactorvr_craft_item s_craft_item = nullptr;
static pfn_xfactorvr_get_resource_patches s_get_resource_patches = nullptr;
static pfn_xfactorvr_connect_rcon s_connect_rcon = nullptr;
static pfn_xfactorvr_is_rcon_connected s_is_rcon_connected = nullptr;
static pfn_xfactorvr_manual_mine s_manual_mine = nullptr;
static pfn_xfactorvr_interact_machine s_interact_machine = nullptr;
static pfn_xfactorvr_reset_world s_reset_world = nullptr;
static pfn_xfactorvr_get_machine_info s_get_machine_info = nullptr;
static pfn_xfactorvr_get_carried_item s_get_carried_item = nullptr;
static pfn_xfactorvr_pan_view s_pan_view = nullptr;
static pfn_xfactorvr_get_view_center s_get_view_center = nullptr;
static pfn_xfactorvr_vr_hand_action s_vr_hand_action = nullptr;

// Estado de Minado Manual con Pico (mantener Trigger en vetas de mineral)
static float s_manual_mining_timer = 0.0f;
static float s_manual_mining_tile_x = 999.0f;
static float s_manual_mining_tile_y = 999.0f;
static float s_last_mining_strike = 0.0f;

// ============================================================================
// PERSISTENCIA DE MESA (LAYOUT.JSON) — GENERALS ZERO HOUR XR PATTERN
// ============================================================================
static const char* LAYOUT_FILE_PATH = "/sdcard/XFactorVR/layout.json";

static void update_table_matrix();

static void save_tabletop_layout() {
    FILE* f = fopen(LAYOUT_FILE_PATH, "w");
    if (!f) return;
    fprintf(f, "{\n  \"pos_x\": %.4f,\n  \"pos_y\": %.4f,\n  \"pos_z\": %.4f,\n  \"yaw\": %.4f,\n  \"scale\": %.4f\n}\n",
        s_table_pos.x, s_table_pos.y, s_table_pos.z, s_table_yaw, s_table_scale);
    fclose(f);
    LOGI("[Layout] Layout guardado en %s (x=%.2f, y=%.2f, z=%.2f, yaw=%.2f, scale=%.2f)",
        LAYOUT_FILE_PATH, s_table_pos.x, s_table_pos.y, s_table_pos.z, s_table_yaw, s_table_scale);
}

static void load_tabletop_layout() {
    FILE* f = fopen(LAYOUT_FILE_PATH, "r");
    if (!f) return;
    float x = 0, y = 0, z = 0, yaw = 0, scale = 1;
    if (fscanf(f, " { \"pos_x\": %f , \"pos_y\": %f , \"pos_z\": %f , \"yaw\": %f , \"scale\": %f }",
               &x, &y, &z, &yaw, &scale) >= 4) {
        s_table_pos.x = x;
        s_table_pos.y = y;
        s_table_pos.z = z;
        s_table_yaw = yaw;
        if (scale > 0.15f && scale < 8.0f) {
            s_table_scale = scale;
        }
        s_table_placed = true;
        update_table_matrix();
        LOGI("[Layout] Layout restaurado desde %s", LAYOUT_FILE_PATH);
    }
    fclose(f);
}

static void update_table_matrix() {
    float rot[16], trans[16], sc[16], model_temp[16];
    mat4_rotate_y(rot, s_table_yaw);
    mat4_translate(trans, s_table_pos.x, s_table_pos.y, s_table_pos.z);
    mat4_scale(sc, s_table_scale, s_table_scale, s_table_scale);
    mat4_multiply(rot, sc, model_temp);
    mat4_multiply(trans, model_temp, s_table_model_mat);
}

// Interacción bimanual (Pinch-to-Zoom y Rotación bimanual)
static bool s_dual_grip_active = false;
static float s_dual_grip_initial_dist = 1.0f;
static float s_dual_grip_initial_scale = 1.0f;
static float s_dual_grip_initial_angle = 0.0f;
static float s_dual_grip_initial_yaw = 0.0f;

struct ModelMesh {
    GLuint vao = 0;
    GLuint vbo = 0;
    int vertex_count = 0;
    float width = 0.1f;
};

static ModelMesh s_entity_meshes[ENTITY_TYPE_COUNT];
static ModelMesh s_turret_top_mesh;

// Herramientas y Construcción 6DoF (Fase 2)
static int s_selected_build_tool = ENTITY_ASSEMBLING_MACHINE;
static float s_placement_yaw = 0.0f;
static int s_hovered_quickbar_slot = -1;
static bool s_stick_rotated = false;

// Construcción por arrastre continuo (drag-to-build).
// Mantener el gatillo y mover el puntero tiende lineas de cintas o postes sin
// tener que hacer clic casilla por casilla.
static int s_last_drag_tile_x = 9999;
static int s_last_drag_tile_y = 9999;

// Herramientas que se extienden arrastrando (cintas, postes y muros).
static bool is_drag_build_tool(int type) {
    return type == ENTITY_TRANSPORT_BELT || type == ENTITY_ELECTRIC_POLE;
}

// Desplazamiento de la ventana de mapa, en casillas.
// Permite explorar la fabrica mas alla del area inicial requestada al servidor.
static float s_view_pan_x = 0.0f;
static float s_view_pan_y = 0.0f;

// Color de cada item (ids de src/game_bridge.rs) para las piezas que viajan
// por las cintas y los insertadores.
static void item_color(uint32_t item_id, float& r, float& g, float& b) {
    switch (item_id) {
        case 1:  r = 0.45f; g = 0.38f; b = 0.32f; break; // mineral de hierro
        case 2:  r = 0.58f; g = 0.38f; b = 0.24f; break; // mineral de cobre
        case 3:  r = 0.14f; g = 0.14f; b = 0.16f; break; // carbon
        case 4:  r = 0.55f; g = 0.53f; b = 0.50f; break; // piedra
        case 5:  r = 0.74f; g = 0.77f; b = 0.80f; break; // plancha de hierro
        case 6:  r = 0.86f; g = 0.50f; b = 0.28f; break; // plancha de cobre
        case 7:  r = 0.60f; g = 0.62f; b = 0.66f; break; // engranaje
        case 8:  r = 0.20f; g = 0.85f; b = 0.32f; break; // circuito electronico
        case 9:  r = 0.25f; g = 0.45f; b = 0.62f; break; // ensambladora
        case 10: r = 0.18f; g = 0.55f; b = 0.50f; break; // perforadora
        case 11: r = 0.50f; g = 0.40f; b = 0.32f; break; // horno
        case 12: r = 0.92f; g = 0.80f; b = 0.18f; break; // cinta
        case 13: r = 0.96f; g = 0.72f; b = 0.12f; break; // insertador
        case 14: r = 0.45f; g = 0.35f; b = 0.22f; break; // poste
        case 15: r = 0.40f; g = 0.42f; b = 0.46f; break; // torreta
        case 16: r = 0.72f; g = 0.74f; b = 0.78f; break; // radar
        case 17: r = 0.60f; g = 0.65f; b = 0.72f; break; // robopuerto
        case 18: r = 0.55f; g = 0.38f; b = 0.22f; break; // cofre
        default: r = 0.85f; g = 0.85f; b = 0.85f; break;
    }
}

// Partículas de humo, vapor, polvo y chispas (Fase 3)
struct Particle {
    float x, y, z;
    float vx, vy, vz;
    float r, g, b, a;
    float size;
    float life;
    float max_life;
};
static std::vector<Particle> s_particles;

static void spawn_particle(float x, float y, float z, float vx, float vy, float vz,
                           float r, float g, float b, float a, float size, float life) {
    if (s_particles.size() >= 256) return;
    s_particles.push_back({x, y, z, vx, vy, vz, r, g, b, a, size, life, life});
}

// Sub-lotes estáticos de la mesa y diorama
static GLuint s_vao_soil = 0, s_vbo_soil = 0;
static int s_count_soil = 0;

static GLuint s_vao_concrete = 0, s_vbo_concrete = 0;
static int s_count_concrete = 0;

static GLuint s_vao_hazard = 0, s_vbo_hazard = 0;
static int s_count_hazard = 0;

static GLuint s_vao_belt = 0, s_vbo_belt = 0;
static int s_count_belt = 0;

static GLuint s_vao_ui = 0, s_vbo_ui = 0;
static int s_count_ui = 0;

// Lote dinámico (Ítems en cinta, cables catenarios, inserters, drones, retículos y partículas)
static GLuint s_dynamic_vao = 0, s_dynamic_vbo = 0;

static bool check_shader_compile(GLuint shader, const char* name) {
    GLint success = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &success);
    if (!success) {
        char info[1024];
        glGetShaderInfoLog(shader, sizeof(info), nullptr, info);
        LOGE("Shader [%s] error de compilación: %s", name, info);
        return false;
    }
    return true;
}

static bool check_program_link(GLuint program) {
    GLint success = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &success);
    if (!success) {
        char info[1024];
        glGetProgramInfoLog(program, sizeof(info), nullptr, info);
        LOGE("Shader program error de enlace: %s", info);
        return false;
    }
    return true;
}

static GLuint create_texture_rgba(int width, int height, const uint32_t* pixels, bool repeat = true) {
    GLuint tex = 0;
    glGenTextures(1, &tex);
    glBindTexture(GL_TEXTURE_2D, tex);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, width, height, 0, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    glGenerateMipmap(GL_TEXTURE_2D);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, repeat ? GL_REPEAT : GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, repeat ? GL_REPEAT : GL_CLAMP_TO_EDGE);
    glBindTexture(GL_TEXTURE_2D, 0);
    return tex;
}

static void init_procedural_textures() {
    // 1. Concreto refinado industrial (256x256): Losas de 40cm con ranuras de dilatación y pernos
    std::vector<uint32_t> conc_pix(256 * 256);
    for (int y = 0; y < 256; ++y) {
        for (int x = 0; x < 256; ++x) {
            int n = ((x * 37 + y * 59 + (x ^ y) * 17) % 23) - 11;
            uint8_t r = (uint8_t)std::clamp(0x4a + n, 0, 255);
            uint8_t g = (uint8_t)std::clamp(0x52 + n, 0, 255);
            uint8_t b = (uint8_t)std::clamp(0x59 + n, 0, 255);

            // Juntas de dilatación oscuras
            if (x < 2 || x > 253 || y < 2 || y > 253 ||
                abs(x - 128) <= 1 || abs(y - 128) <= 1) {
                r = 0x20; g = 0x24; b = 0x27;
            }

            // Pernos de anclaje de acero en esquinas
            int lx = x % 128, ly = y % 128;
            if ((lx >= 8 && lx <= 12 && ly >= 8 && ly <= 12) ||
                (lx >= 115 && lx <= 119 && ly >= 8 && ly <= 12) ||
                (lx >= 8 && lx <= 12 && ly >= 115 && ly <= 119) ||
                (lx >= 115 && lx <= 119 && ly >= 115 && ly <= 119)) {
                r = 0x8a; g = 0x93; b = 0x9c;
            }
            conc_pix[y * 256 + x] = (0xff << 24) | (b << 16) | (g << 8) | r;
        }
    }
    s_tex_concrete = create_texture_rgba(256, 256, conc_pix.data(), true);

    // 2. Franjas de peligro Factorio (Hazard stripes 128x128): Amarillo industrial y gris carbón
    std::vector<uint32_t> haz_pix(128 * 128);
    for (int y = 0; y < 128; ++y) {
        for (int x = 0; x < 128; ++x) {
            int diag = (x + y) / 16;
            uint8_t r, g, b;
            if (diag % 2 == 0) {
                r = 0xf5; g = 0xb0; b = 0x18; // Amarillo Factorio
            } else {
                r = 0x22; g = 0x26; b = 0x2b; // Acero carbón oscuro
            }
            if (((x + y) % 16) == 0) {
                r = (uint8_t)(r * 0.7f); g = (uint8_t)(g * 0.7f); b = (uint8_t)(b * 0.7f);
            }
            haz_pix[y * 128 + x] = (0xff << 24) | (b << 16) | (g << 8) | r;
        }
    }
    s_tex_hazard = create_texture_rgba(128, 128, haz_pix.data(), true);

    // 3. Cinta transportadora (64x128): Rodillos metálicos con flechas de avance amarillas (>>>)
    std::vector<uint32_t> belt_pix(64 * 128);
    for (int y = 0; y < 128; ++y) {
        for (int x = 0; x < 64; ++x) {
            uint8_t r, g, b;
            if (x < 8 || x > 55) {
                r = 0x1c; g = 0x20; b = 0x24; // Guía lateral de chapa de acero
            } else {
                int mod_y = y % 32;
                r = 0x2e; g = 0x33; b = 0x39; // Base del rodillo
                if (mod_y == 0 || mod_y == 1) {
                    r = 0x18; g = 0x1b; b = 0x1e;
                }
                int dist_center = abs(x - 32);
                if (dist_center < 16) {
                    int arrow_peak = 22 - (dist_center / 2);
                    if (abs(mod_y - arrow_peak) <= 2) {
                        r = 0xf0; g = 0xb4; b = 0x1a; // Flecha chevron amarilla
                    }
                }
            }
            belt_pix[y * 64 + x] = (0xff << 24) | (b << 16) | (g << 8) | r;
        }
    }
    s_tex_belt = create_texture_rgba(64, 128, belt_pix.data(), true);

    // 4. Estratos geológicos subterráneos del plinto (128x128)
    std::vector<uint32_t> soil_pix(128 * 128);
    for (int y = 0; y < 128; ++y) {
        for (int x = 0; x < 128; ++x) {
            int n = ((x * 47) ^ (y * 79)) % 14;
            uint8_t r, g, b;
            if (y < 22) {
                r = 0x3a + n; g = 0x2d + n; b = 0x20 + n;
            } else if (y < 70) {
                r = 0x58 + n; g = 0x43 + n; b = 0x31 + n;
            } else {
                r = 0x2a + n; g = 0x2d + n; b = 0x32 + n;
            }
            soil_pix[y * 128 + x] = (0xff << 24) | (b << 16) | (g << 8) | r;
        }
    }
    s_tex_soil = create_texture_rgba(128, 128, soil_pix.data(), true);

    // 5. Acero industrial cepillado (64x64)
    std::vector<uint32_t> steel_pix(64 * 64);
    for (int y = 0; y < 64; ++y) {
        for (int x = 0; x < 64; ++x) {
            int n = (x * 19) % 15;
            uint8_t r = 0x3a + n;
            uint8_t g = 0x44 + n;
            uint8_t b = 0x4e + n;
            steel_pix[y * 64 + x] = (0xff << 24) | (b << 16) | (g << 8) | r;
        }
    }
    s_tex_steel = create_texture_rgba(64, 64, steel_pix.data(), true);
}

// Generadores de geometría con normales, UVs y color
static void add_box_textured(std::vector<DioramaVertex>& out,
                             float x, float y, float z,
                             float w, float h, float d,
                             float u_scale, float v_scale,
                             float r, float g, float b, float a) {
    float hw = w * 0.5f, hh = h * 0.5f, hd = d * 0.5f;

    // +Y (Top)
    out.push_back({x - hw, y + hh, z - hd, 0, 1, 0, 0, 0, r, g, b, a});
    out.push_back({x - hw, y + hh, z + hd, 0, 1, 0, 0, v_scale, r, g, b, a});
    out.push_back({x + hw, y + hh, z + hd, 0, 1, 0, u_scale, v_scale, r, g, b, a});
    out.push_back({x - hw, y + hh, z - hd, 0, 1, 0, 0, 0, r, g, b, a});
    out.push_back({x + hw, y + hh, z + hd, 0, 1, 0, u_scale, v_scale, r, g, b, a});
    out.push_back({x + hw, y + hh, z - hd, 0, 1, 0, u_scale, 0, r, g, b, a});

    // -Y (Bottom)
    out.push_back({x - hw, y - hh, z - hd, 0, -1, 0, 0, 0, r * 0.6f, g * 0.6f, b * 0.6f, a});
    out.push_back({x + hw, y - hh, z - hd, 0, -1, 0, u_scale, 0, r * 0.6f, g * 0.6f, b * 0.6f, a});
    out.push_back({x + hw, y - hh, z + hd, 0, -1, 0, u_scale, v_scale, r * 0.6f, g * 0.6f, b * 0.6f, a});
    out.push_back({x - hw, y - hh, z - hd, 0, -1, 0, 0, 0, r * 0.6f, g * 0.6f, b * 0.6f, a});
    out.push_back({x + hw, y - hh, z + hd, 0, -1, 0, u_scale, v_scale, r * 0.6f, g * 0.6f, b * 0.6f, a});
    out.push_back({x - hw, y - hh, z + hd, 0, -1, 0, 0, v_scale, r * 0.6f, g * 0.6f, b * 0.6f, a});

    // +Z (Front)
    out.push_back({x - hw, y - hh, z + hd, 0, 0, 1, 0, 0, r * 0.9f, g * 0.9f, b * 0.9f, a});
    out.push_back({x + hw, y - hh, z + hd, 0, 0, 1, u_scale, 0, r * 0.9f, g * 0.9f, b * 0.9f, a});
    out.push_back({x + hw, y + hh, z + hd, 0, 0, 1, u_scale, v_scale, r * 0.9f, g * 0.9f, b * 0.9f, a});
    out.push_back({x - hw, y - hh, z + hd, 0, 0, 1, 0, 0, r * 0.9f, g * 0.9f, b * 0.9f, a});
    out.push_back({x + hw, y + hh, z + hd, 0, 0, 1, u_scale, v_scale, r * 0.9f, g * 0.9f, b * 0.9f, a});
    out.push_back({x - hw, y + hh, z + hd, 0, 0, 1, 0, v_scale, r * 0.9f, g * 0.9f, b * 0.9f, a});

    // -Z (Back)
    out.push_back({x - hw, y - hh, z - hd, 0, 0, -1, 0, 0, r * 0.7f, g * 0.7f, b * 0.7f, a});
    out.push_back({x - hw, y + hh, z - hd, 0, 0, -1, 0, v_scale, r * 0.7f, g * 0.7f, b * 0.7f, a});
    out.push_back({x + hw, y + hh, z - hd, 0, 0, -1, u_scale, v_scale, r * 0.7f, g * 0.7f, b * 0.7f, a});
    out.push_back({x - hw, y - hh, z - hd, 0, 0, -1, 0, 0, r * 0.7f, g * 0.7f, b * 0.7f, a});
    out.push_back({x + hw, y + hh, z - hd, 0, 0, -1, u_scale, v_scale, r * 0.7f, g * 0.7f, b * 0.7f, a});
    out.push_back({x + hw, y - hh, z - hd, 0, 0, -1, u_scale, 0, r * 0.7f, g * 0.7f, b * 0.7f, a});

    // -X (Left)
    out.push_back({x - hw, y - hh, z - hd, -1, 0, 0, 0, 0, r * 0.8f, g * 0.8f, b * 0.8f, a});
    out.push_back({x - hw, y - hh, z + hd, -1, 0, 0, u_scale, 0, r * 0.8f, g * 0.8f, b * 0.8f, a});
    out.push_back({x - hw, y + hh, z + hd, -1, 0, 0, u_scale, v_scale, r * 0.8f, g * 0.8f, b * 0.8f, a});
    out.push_back({x - hw, y - hh, z - hd, -1, 0, 0, 0, 0, r * 0.8f, g * 0.8f, b * 0.8f, a});
    out.push_back({x - hw, y + hh, z + hd, -1, 0, 0, u_scale, v_scale, r * 0.8f, g * 0.8f, b * 0.8f, a});
    out.push_back({x - hw, y + hh, z - hd, -1, 0, 0, 0, v_scale, r * 0.8f, g * 0.8f, b * 0.8f, a});

    // +X (Right)
    out.push_back({x + hw, y - hh, z - hd, 1, 0, 0, 0, 0, r * 0.85f, g * 0.85f, b * 0.85f, a});
    out.push_back({x + hw, y + hh, z - hd, 1, 0, 0, 0, v_scale, r * 0.85f, g * 0.85f, b * 0.85f, a});
    out.push_back({x + hw, y + hh, z + hd, 1, 0, 0, u_scale, v_scale, r * 0.85f, g * 0.85f, b * 0.85f, a});
    out.push_back({x + hw, y - hh, z - hd, 1, 0, 0, 0, 0, r * 0.85f, g * 0.85f, b * 0.85f, a});
    out.push_back({x + hw, y + hh, z + hd, 1, 0, 0, u_scale, v_scale, r * 0.85f, g * 0.85f, b * 0.85f, a});
    out.push_back({x + hw, y - hh, z + hd, 1, 0, 0, u_scale, 0, r * 0.85f, g * 0.85f, b * 0.85f, a});
}

static void add_cylinder_textured(std::vector<DioramaVertex>& out,
                                 float x, float y, float z,
                                 float radius, float h,
                                 float r, float g, float b, float a,
                                 int sides = 12) {
    float hh = h * 0.5f;
    for (int i = 0; i < sides; ++i) {
        float a0 = (float)i * 6.2831853f / (float)sides;
        float a1 = (float)(i + 1) * 6.2831853f / (float)sides;
        float c0 = cosf(a0), s0 = sinf(a0);
        float c1 = cosf(a1), s1 = sinf(a1);
        float u0 = (float)i / (float)sides;
        float u1 = (float)(i + 1) / (float)sides;

        out.push_back({x + c0 * radius, y - hh, z + s0 * radius, c0, 0, s0, u0, 0, r, g, b, a});
        out.push_back({x + c1 * radius, y - hh, z + s1 * radius, c1, 0, s1, u1, 0, r, g, b, a});
        out.push_back({x + c1 * radius, y + hh, z + s1 * radius, c1, 0, s1, u1, 1, r, g, b, a});

        out.push_back({x + c0 * radius, y - hh, z + s0 * radius, c0, 0, s0, u0, 0, r, g, b, a});
        out.push_back({x + c1 * radius, y + hh, z + s1 * radius, c1, 0, s1, u1, 1, r, g, b, a});
        out.push_back({x + c0 * radius, y + hh, z + s0 * radius, c0, 0, s0, u0, 1, r, g, b, a});

        out.push_back({x, y + hh, z, 0, 1, 0, 0.5f, 0.5f, r, g, b, a});
        out.push_back({x + c0 * radius, y + hh, z + s0 * radius, 0, 1, 0, 0.5f + c0 * 0.5f, 0.5f + s0 * 0.5f, r, g, b, a});
        out.push_back({x + c1 * radius, y + hh, z + s1 * radius, 0, 1, 0, 0.5f + c1 * 0.5f, 0.5f + s1 * 0.5f, r, g, b, a});
    }
}

// Generador de cables catenarios curvados colgantes
static void add_catenary_cable(std::vector<DioramaVertex>& out,
                               float x0, float y0, float z0,
                               float x1, float y1, float z1,
                               float sag, int segments,
                               float r, float g, float b, float a) {
    for (int i = 0; i < segments; ++i) {
        float t0 = (float)i / (float)segments;
        float t1 = (float)(i + 1) / (float)segments;

        float px0 = x0 + (x1 - x0) * t0;
        float pz0 = z0 + (z1 - z0) * t0;
        float py0 = y0 + (y1 - y0) * t0 - 4.0f * sag * t0 * (1.0f - t0);

        float px1 = x0 + (x1 - x0) * t1;
        float pz1 = z0 + (z1 - z0) * t1;
        float py1 = y0 + (y1 - y0) * t1 - 4.0f * sag * t1 * (1.0f - t1);

        add_box_textured(out, (px0 + px1) * 0.5f, (py0 + py1) * 0.5f, (pz0 + pz1) * 0.5f,
                         0.003f, 0.003f, 0.02f, 1.0f, 1.0f, r, g, b, a);
    }
}

static GLuint create_mesh_vao(const std::vector<DioramaVertex>& verts, GLuint& out_vbo) {
    GLuint vao = 0;
    glGenVertexArrays(1, &vao);
    glBindVertexArray(vao);
    glGenBuffers(1, &out_vbo);
    glBindBuffer(GL_ARRAY_BUFFER, out_vbo);
    glBufferData(GL_ARRAY_BUFFER, verts.size() * sizeof(DioramaVertex), verts.data(), GL_STATIC_DRAW);

    glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, sizeof(DioramaVertex), (void*)0);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(1, 3, GL_FLOAT, GL_FALSE, sizeof(DioramaVertex), (void*)(3 * sizeof(float)));
    glEnableVertexAttribArray(1);
    glVertexAttribPointer(2, 2, GL_FLOAT, GL_FALSE, sizeof(DioramaVertex), (void*)(6 * sizeof(float)));
    glEnableVertexAttribArray(2);
    glVertexAttribPointer(3, 4, GL_FLOAT, GL_FALSE, sizeof(DioramaVertex), (void*)(8 * sizeof(float)));
    glEnableVertexAttribArray(3);

    glBindVertexArray(0);
    glBindBuffer(GL_ARRAY_BUFFER, 0);
    return vao;
}

// --- Atlas de sprites reales de Factorio -------------------------------------
// Los pixeles llegan en RGBA8 crudo desde Rust: el extractor escribe el atlas
// en crudo porque en Android no hay decodificador de PNG, y lo parte en paginas
// porque una textura de mas de 4096 px de alto no cabe en la GPU movil.
static bool ensure_atlas_texture() {
    if (s_atlas_probed) return s_atlas_page_count > 0;
    s_atlas_probed = true;
    if (!s_get_atlas_page_count || !s_get_atlas_page_size || !s_get_atlas_page_pixels) return false;

    int pages = s_get_atlas_page_count();
    if (pages <= 0 || pages > XFACTORVR_MAX_ATLAS_PAGES) {
        LOGI("openxr_engine: no hay atlas de sprites (paginas=%d)", pages);
        return false;
    }

    for (int p = 0; p < pages; ++p) {
        int w = 0, h = 0;
        if (!s_get_atlas_page_size((uint32_t)p, &w, &h) || w <= 0 || h <= 0) continue;
        size_t bytes = (size_t)w * (size_t)h * 4u;
        std::vector<uint8_t> pixels(bytes);
        int written = s_get_atlas_page_pixels((uint32_t)p, pixels.data(), (int)bytes);
        if (written <= 0) continue;

        GLuint tex = 0;
        glGenTextures(1, &tex);
        glBindTexture(GL_TEXTURE_2D, tex);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, pixels.data());
        s_tex_atlas[p] = tex;
        s_atlas_page_w[p] = w;
        s_atlas_page_h[p] = h;
        s_atlas_page_count = p + 1;
    }
    LOGI("openxr_engine: atlas de sprites de Factorio cargado (%d paginas)", s_atlas_page_count);
    return s_atlas_page_count > 0;
}

// --- Atlas del suelo real ----------------------------------------------------
// El suelo del mapa (hierba, tierra, arena...) son celdas de 32x32 recortadas
// de las hojas de terreno del juego.  Van en su propio atlas, paginado por el
// mismo motivo que los sprites.
static bool ensure_terrain_texture() {
    if (s_terrain_probed) return s_terrain_page_count > 0;
    s_terrain_probed = true;
    if (!s_get_terrain_page_count || !s_get_terrain_page_size || !s_get_terrain_page_pixels) return false;

    int pages = s_get_terrain_page_count();
    if (pages <= 0 || pages > XFACTORVR_MAX_ATLAS_PAGES) {
        LOGI("openxr_engine: no hay atlas de suelo (paginas=%d)", pages);
        return false;
    }

    for (int p = 0; p < pages; ++p) {
        int w = 0, h = 0;
        if (!s_get_terrain_page_size((uint32_t)p, &w, &h) || w <= 0 || h <= 0) continue;
        size_t bytes = (size_t)w * (size_t)h * 4u;
        std::vector<uint8_t> pixels(bytes);
        int written = s_get_terrain_page_pixels((uint32_t)p, pixels.data(), (int)bytes);
        if (written <= 0) continue;

        GLuint tex = 0;
        glGenTextures(1, &tex);
        glBindTexture(GL_TEXTURE_2D, tex);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, pixels.data());
        s_tex_terrain[p] = tex;
        s_terrain_page_w[p] = w;
        s_terrain_page_h[p] = h;
        s_terrain_page_count = p + 1;
    }
    LOGI("openxr_engine: atlas de suelo cargado (%d paginas)", s_terrain_page_count);
    return s_terrain_page_count > 0;
}

// Anade al lote un quad plano: el sprite tumbado sobre la mesa. El arte de
// Factorio es una vista cenital, asi que tumbado es la proyeccion correcta.
static void add_sprite_quad(std::vector<DioramaVertex>& verts,
                            float cx, float cz, float y,
                            float half_w, float half_d,
                            float u0, float v0, float u1, float v1,
                            float r, float g, float b, float a) {
    const float xs[4] = { cx + half_w, cx + half_w, cx - half_w, cx - half_w };
    const float zs[4] = { cz - half_d, cz + half_d, cz + half_d, cz - half_d };
    const float us[4] = { u1, u1, u0, u0 };
    const float vs[4] = { v0, v1, v1, v0 };
    static const int idx[6] = { 0, 1, 2, 0, 2, 3 };
    for (int k = 0; k < 6; ++k) {
        int i = idx[k];
        DioramaVertex v;
        v.x = xs[i];
        v.y = y;
        v.z = zs[i];
        v.nx = 0.0f;
        v.ny = 1.0f;
        v.nz = 0.0f;
        v.u = us[i];
        v.v = vs[i];
        v.r = r;
        v.g = g;
        v.b = b;
        v.a = a;
        verts.push_back(v);
    }
}

// Sube el lote de sprites de una pagina a la GPU y lo dibuja tumbado sobre la
// mesa. Cada pagina del atlas es una textura y se pinta por separado.
static void draw_sprite_batch(SpriteBatch& batch, uint32_t page,
                              const float* model, const float* mvp,
                              const GLuint* texturas = nullptr) {
    if (batch.verts.empty() || page >= (uint32_t)s_atlas_page_count) return;
    const GLuint* tex = texturas ? texturas : s_tex_atlas;

    if (batch.vao == 0) {
        batch.vao = create_mesh_vao(batch.verts, batch.vbo);
    } else {
        glBindBuffer(GL_ARRAY_BUFFER, batch.vbo);
        glBufferData(GL_ARRAY_BUFFER, batch.verts.size() * sizeof(DioramaVertex),
                     batch.verts.data(), GL_DYNAMIC_DRAW);
    }
    batch.vertex_count = (int)batch.verts.size();

    glUseProgram(s_program);
    glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, model);
    glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, mvp);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, tex[page]);
    glUniform1i(s_tex_loc, 0);
    glUniform1f(s_use_tex_loc, 1.0f);
    glUniform1f(s_lit_loc, 1.0f);

    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    glDepthMask(GL_FALSE);
    glBindVertexArray(batch.vao);
    glDrawArrays(GL_TRIANGLES, 0, batch.vertex_count);
    glDepthMask(GL_TRUE);
    glDisable(GL_BLEND);

    glUniform1f(s_use_tex_loc, 0.0f);
    glBindVertexArray(0);
}

// Malla del suelo real: un quad por casilla con la textura del terreno.
//
// La geometria la calcula Rust (src/terreno.rs), que es donde estan el
// snapshot del terreno y el atlas: asi se puede probar con cargo test sin
// necesitar el visor.  Aqui solo se pinta.
static void build_terrain_mesh(float pan_x, float pan_y) {
    for (int p = 0; p < XFACTORVR_MAX_ATLAS_PAGES; ++p) {
        s_terrain_batches[p].verts.clear();
    }
    if (!s_get_terrain_mesh || !ensure_terrain_texture()) return;

    // Cada celda: tile_x, tile_y, u0, v0, u1, v1, pagina.
    const int MAX_CELLS = 256 * 256;
    std::vector<float> datos((size_t)MAX_CELLS * 7);
    int celdas = s_get_terrain_mesh(datos.data(), (int)datos.size());
    if (celdas <= 0) return;

    for (int i = 0; i < celdas; ++i) {
        const float* v = &datos[(size_t)i * 7];
        int page = (int)v[6];
        if (page < 0 || page >= XFACTORVR_MAX_ATLAS_PAGES) continue;
        if (s_terrain_page_w[page] <= 0 || s_terrain_page_h[page] <= 0) continue;
        // Una casilla son 32 px en el juego y 0.04 m en la mesa.
        float mx = (v[0] - pan_x) * 0.04f;
        float mz = (v[1] - pan_y) * 0.04f;
        // El suelo va justo encima del suelo de concreto de la mesa.
        add_sprite_quad(s_terrain_batches[page].verts, mx, mz, 0.0009f,
                        0.02f, 0.02f, v[2], v[3], v[4], v[5],
                        1.0f, 1.0f, 1.0f, 1.0f);
    }
}


static void init_glb_loader() {
    if (s_load_glb && s_get_live_entities) return;
    void* handle = RTLD_DEFAULT;
    s_load_glb = (pfn_xfactorvr_load_glb)dlsym(handle, "xfactorvr_load_glb");
    s_free_glb = (pfn_xfactorvr_free_glb)dlsym(handle, "xfactorvr_free_glb");
    s_get_live_entities = (pfn_xfactorvr_get_live_entities)dlsym(handle, "xfactorvr_get_live_entities");
    s_place_or_update_entity = (pfn_xfactorvr_place_or_update_entity)dlsym(handle, "xfactorvr_place_or_update_entity");
    s_mine_entity = (pfn_xfactorvr_mine_entity)dlsym(handle, "xfactorvr_mine_entity");
    s_update_simulation = (pfn_xfactorvr_update_simulation)dlsym(handle, "xfactorvr_update_simulation");
    s_get_player_inventory = (pfn_xfactorvr_get_player_inventory)dlsym(handle, "xfactorvr_get_player_inventory");
    s_craft_item = (pfn_xfactorvr_craft_item)dlsym(handle, "xfactorvr_craft_item");
    s_get_resource_patches = (pfn_xfactorvr_get_resource_patches)dlsym(handle, "xfactorvr_get_resource_patches");
    s_connect_rcon = (pfn_xfactorvr_connect_rcon)dlsym(handle, "xfactorvr_connect_rcon");
    s_is_rcon_connected = (pfn_xfactorvr_is_rcon_connected)dlsym(handle, "xfactorvr_is_rcon_connected");
    s_manual_mine = (pfn_xfactorvr_manual_mine)dlsym(handle, "xfactorvr_manual_mine");
    s_interact_machine = (pfn_xfactorvr_interact_machine)dlsym(handle, "xfactorvr_interact_machine");
    s_reset_world = (pfn_xfactorvr_reset_world)dlsym(handle, "xfactorvr_reset_world");
    s_get_machine_info = (pfn_xfactorvr_get_machine_info)dlsym(handle, "xfactorvr_get_machine_info");
    s_get_carried_item = (pfn_xfactorvr_get_carried_item)dlsym(handle, "xfactorvr_get_carried_item");
    s_pan_view = (pfn_xfactorvr_pan_view)dlsym(handle, "xfactorvr_pan_view");
    s_get_view_center = (pfn_xfactorvr_get_view_center)dlsym(handle, "xfactorvr_get_view_center");
    s_vr_hand_action = (pfn_xfactorvr_vr_hand_action)dlsym(handle, "xfactorvr_vr_hand_action");
    s_get_atlas_page_count = (pfn_xfactorvr_get_atlas_page_count)dlsym(handle, "xfactorvr_get_atlas_page_count");
    s_get_atlas_page_size = (pfn_xfactorvr_get_atlas_page_size)dlsym(handle, "xfactorvr_get_atlas_page_size");
    s_get_atlas_page_pixels = (pfn_xfactorvr_get_atlas_page_pixels)dlsym(handle, "xfactorvr_get_atlas_page_pixels");
    s_get_sprite_frame = (pfn_xfactorvr_get_sprite_frame)dlsym(handle, "xfactorvr_get_sprite_frame");
    s_get_terrain_page_count = (pfn_xfactorvr_get_terrain_page_count)dlsym(handle, "xfactorvr_get_terrain_page_count");
    s_get_terrain_page_size = (pfn_xfactorvr_get_terrain_page_size)dlsym(handle, "xfactorvr_get_terrain_page_size");
    s_get_terrain_page_pixels = (pfn_xfactorvr_get_terrain_page_pixels)dlsym(handle, "xfactorvr_get_terrain_page_pixels");
    s_get_terrain_tile_size = (pfn_xfactorvr_get_terrain_tile_size)dlsym(handle, "xfactorvr_get_terrain_tile_size");
    s_get_terrain_variant_count = (pfn_xfactorvr_get_terrain_variant_count)dlsym(handle, "xfactorvr_get_terrain_variant_count");
    s_get_terrain_tile = (pfn_xfactorvr_get_terrain_tile)dlsym(handle, "xfactorvr_get_terrain_tile");
    s_get_terrain_mesh = (pfn_xfactorvr_get_terrain_mesh)dlsym(handle, "xfactorvr_get_terrain_mesh");

    if (!s_load_glb || !s_get_live_entities) {
        void* mod_handle = dlopen("libxfactorvr.so", RTLD_NOW | RTLD_GLOBAL);
        if (mod_handle) {
            s_load_glb = (pfn_xfactorvr_load_glb)dlsym(mod_handle, "xfactorvr_load_glb");
            s_free_glb = (pfn_xfactorvr_free_glb)dlsym(mod_handle, "xfactorvr_free_glb");
            s_get_live_entities = (pfn_xfactorvr_get_live_entities)dlsym(mod_handle, "xfactorvr_get_live_entities");
            s_place_or_update_entity = (pfn_xfactorvr_place_or_update_entity)dlsym(mod_handle, "xfactorvr_place_or_update_entity");
            s_mine_entity = (pfn_xfactorvr_mine_entity)dlsym(mod_handle, "xfactorvr_mine_entity");
            s_update_simulation = (pfn_xfactorvr_update_simulation)dlsym(mod_handle, "xfactorvr_update_simulation");
            s_get_player_inventory = (pfn_xfactorvr_get_player_inventory)dlsym(mod_handle, "xfactorvr_get_player_inventory");
            s_craft_item = (pfn_xfactorvr_craft_item)dlsym(mod_handle, "xfactorvr_craft_item");
            s_get_resource_patches = (pfn_xfactorvr_get_resource_patches)dlsym(mod_handle, "xfactorvr_get_resource_patches");
            s_connect_rcon = (pfn_xfactorvr_connect_rcon)dlsym(mod_handle, "xfactorvr_connect_rcon");
            s_is_rcon_connected = (pfn_xfactorvr_is_rcon_connected)dlsym(mod_handle, "xfactorvr_is_rcon_connected");
            s_manual_mine = (pfn_xfactorvr_manual_mine)dlsym(mod_handle, "xfactorvr_manual_mine");
            s_interact_machine = (pfn_xfactorvr_interact_machine)dlsym(mod_handle, "xfactorvr_interact_machine");
            s_reset_world = (pfn_xfactorvr_reset_world)dlsym(mod_handle, "xfactorvr_reset_world");
            s_get_machine_info = (pfn_xfactorvr_get_machine_info)dlsym(mod_handle, "xfactorvr_get_machine_info");
            s_get_carried_item = (pfn_xfactorvr_get_carried_item)dlsym(mod_handle, "xfactorvr_get_carried_item");
            s_pan_view = (pfn_xfactorvr_pan_view)dlsym(mod_handle, "xfactorvr_pan_view");
            s_get_view_center = (pfn_xfactorvr_get_view_center)dlsym(mod_handle, "xfactorvr_get_view_center");
            s_vr_hand_action = (pfn_xfactorvr_vr_hand_action)dlsym(mod_handle, "xfactorvr_vr_hand_action");
        }
    }
    LOGI("openxr_engine: Cargador GLB y FFI Factorio enlazados (load=%p, get=%p, inv=%p, patches=%p, mine=%p, interact=%p)",
         s_load_glb, s_get_live_entities, s_get_player_inventory, s_get_resource_patches, s_manual_mine, s_interact_machine);

    // Conectar con el motor real de Factorio. Sin esta llamada el puente RCON
    // queda como codigo muerto y la mesa solo muestra la simulacion local.
    // El servidor headless corre dentro del propio visor en 127.0.0.1:27015.
    if (s_connect_rcon) {
        bool linked = s_connect_rcon("127.0.0.1", 27015, "xfactorvr");
        LOGI("openxr_engine: Conexion RCON con Factorio %s", linked ? "ESTABLECIDA" : "fallida (se reintentara)");
    }

    // Restaurar layout persistente si existe
    load_tabletop_layout();
}

static bool load_mesh_from_glb(ModelMesh& out_mesh, const char* path, float target_width) {
    init_glb_loader();
    if (!s_load_glb) return false;

    DioramaVertex* raw_verts = nullptr;
    int count = 0;
    if (!s_load_glb(path, target_width, &raw_verts, &count) || !raw_verts || count <= 0) {
        LOGE("openxr_engine: Fallo al cargar GLB: %s", path);
        return false;
    }

    std::vector<DioramaVertex> verts(raw_verts, raw_verts + count);
    if (s_free_glb) {
        s_free_glb(raw_verts, count);
    }

    out_mesh.vao = create_mesh_vao(verts, out_mesh.vbo);
    out_mesh.vertex_count = (int)verts.size();
    out_mesh.width = target_width;
    LOGI("openxr_engine: Modelo GLB cargado individualmente %s (%d verts)", path, count);
    return true;
}

static void load_or_create_fallback(int type, const char* glb_path, float target_width) {
    if (glb_path && load_mesh_from_glb(s_entity_meshes[type], glb_path, target_width)) {
        return;
    }
    std::vector<DioramaVertex> verts;
    switch (type) {
        case ENTITY_ASSEMBLING_MACHINE:
            add_box_textured(verts, 0, 0.045f, 0, 0.18f, 0.09f, 0.18f, 2.0f, 1.0f, 0.28f, 0.48f, 0.65f, 1.0f);
            add_box_textured(verts, 0, 0.10f, 0, 0.14f, 0.03f, 0.14f, 1.0f, 1.0f, 0.20f, 0.25f, 0.30f, 1.0f);
            add_cylinder_textured(verts, -0.045f, 0.13f, -0.045f, 0.016f, 0.06f, 0.40f, 0.44f, 0.48f, 1.0f);
            break;
        case ENTITY_MINING_DRILL:
            add_box_textured(verts, 0, 0.11f, 0, 0.14f, 0.06f, 0.14f, 1.0f, 1.0f, 0.15f, 0.60f, 0.55f, 1.0f);
            break;
        case ENTITY_FURNACE:
            add_box_textured(verts, 0, 0.05f, 0, 0.12f, 0.10f, 0.12f, 1.0f, 1.0f, 0.48f, 0.46f, 0.44f, 1.0f);
            add_cylinder_textured(verts, 0, 0.12f, 0, 0.022f, 0.05f, 0.35f, 0.36f, 0.38f, 1.0f);
            break;
        case ENTITY_RADAR:
            add_box_textured(verts, 0, 0.035f, 0, 0.10f, 0.07f, 0.10f, 1.0f, 1.0f, 0.30f, 0.34f, 0.38f, 1.0f);
            break;
        case ENTITY_ROBOPORT:
            add_box_textured(verts, 0, 0.05f, 0, 0.16f, 0.10f, 0.16f, 1.0f, 1.0f, 0.72f, 0.45f, 0.22f, 1.0f);
            break;
        case ENTITY_GUN_TURRET:
            add_cylinder_textured(verts, 0, 0.02f, 0, 0.045f, 0.04f, 0.40f, 0.44f, 0.46f, 1.0f, 8);
            break;
        case ENTITY_CHEST:
            add_box_textured(verts, 0, 0.035f, 0, 0.07f, 0.07f, 0.07f, 1.0f, 1.0f, 0.60f, 0.65f, 0.72f, 1.0f);
            break;
        case ENTITY_ELECTRIC_POLE:
            add_cylinder_textured(verts, 0, 0.11f, 0, 0.008f, 0.22f, 0.45f, 0.36f, 0.28f, 1.0f, 8);
            break;
        case ENTITY_TRANSPORT_BELT:
            add_box_textured(verts, 0, 0.006f, 0, 0.04f, 0.010f, 0.04f, 1.0f, 1.0f, 0.35f, 0.38f, 0.40f, 1.0f);
            break;
        case ENTITY_BITER:
            add_box_textured(verts, 0, 0.03f, 0, 0.08f, 0.05f, 0.08f, 1.0f, 1.0f, 0.65f, 0.18f, 0.20f, 1.0f);
            break;
        case ENTITY_TREE:
            add_cylinder_textured(verts, 0, 0.04f, 0, 0.012f, 0.08f, 0.4f, 0.25f, 0.15f, 1.0f);
            add_box_textured(verts, 0, 0.09f, 0, 0.09f, 0.08f, 0.09f, 1.0f, 1.0f, 0.18f, 0.65f, 0.22f, 1.0f);
            break;
        case ENTITY_PLAYER:
            add_box_textured(verts, 0, 0.036f, 0, 0.026f, 0.028f, 0.018f, 1.0f, 1.0f, 0.90f, 0.50f, 0.12f, 1.0f);
            break;
        default:
            add_box_textured(verts, 0, 0.03f, 0, 0.06f, 0.06f, 0.06f, 1.0f, 1.0f, 0.8f, 0.8f, 0.8f, 1.0f);
            break;
    }
    s_entity_meshes[type].vao = create_mesh_vao(verts, s_entity_meshes[type].vbo);
    s_entity_meshes[type].vertex_count = (int)verts.size();
    s_entity_meshes[type].width = target_width;
}

static void init_tabletop_scene() {
    GLuint vs = glCreateShader(GL_VERTEX_SHADER);
    glShaderSource(vs, 1, &VERTEX_SHADER, nullptr);
    glCompileShader(vs);
    check_shader_compile(vs, "VERTEX");

    GLuint fs = glCreateShader(GL_FRAGMENT_SHADER);
    glShaderSource(fs, 1, &FRAGMENT_SHADER, nullptr);
    glCompileShader(fs);
    check_shader_compile(fs, "FRAGMENT");

    s_program = glCreateProgram();
    glAttachShader(s_program, vs);
    glAttachShader(s_program, fs);
    glLinkProgram(s_program);
    check_program_link(s_program);

    s_mvp_loc = glGetUniformLocation(s_program, "u_mvp");
    s_model_loc = glGetUniformLocation(s_program, "u_model");
    s_uv_offset_loc = glGetUniformLocation(s_program, "u_uv_offset");
    s_lit_loc = glGetUniformLocation(s_program, "u_lit");
    s_emissive_loc = glGetUniformLocation(s_program, "u_emissive");
    s_board_bounds_loc = glGetUniformLocation(s_program, "u_board_bounds");
    s_enable_clipping_loc = glGetUniformLocation(s_program, "u_enable_clipping");
    s_tex_loc = glGetUniformLocation(s_program, "u_tex");
    s_use_tex_loc = glGetUniformLocation(s_program, "u_use_tex");

    init_procedural_textures();

    float hw = TABLE_HALF_W;
    float hd = TABLE_HALF_D;

    // A) Base terrenal / Plinto subterráneo estratigráfico (Generals XR pattern)
    std::vector<DioramaVertex> soil_verts;
    add_box_textured(soil_verts, 0.0f, -0.024f, 0.0f, hw * 2.03f, 0.046f, hd * 2.03f, 4.0f, 1.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    s_vao_soil = create_mesh_vao(soil_verts, s_vbo_soil);
    s_count_soil = (int)soil_verts.size();

    // B) Suelo industrial de Concreto Refinado Factorio (Placas de 40cm con dilatación)
    std::vector<DioramaVertex> concrete_verts;
    add_box_textured(concrete_verts, 0.0f, -0.001f, 0.0f, hw * 2.0f, 0.003f, hd * 2.0f, 6.0f, 4.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    s_vao_concrete = create_mesh_vao(concrete_verts, s_vbo_concrete);
    s_count_concrete = (int)concrete_verts.size();

    // C) Franjas de advertencia perimetral (Hazard Stripes) a 45 grados en los 4 bordes
    std::vector<DioramaVertex> hazard_verts;
    float b_thick = 0.035f;
    float b_h = 0.004f;
    add_box_textured(hazard_verts, 0.0f, b_h * 0.5f, -hd + b_thick * 0.5f, hw * 2.0f, b_h, b_thick, 12.0f, 1.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    add_box_textured(hazard_verts, 0.0f, b_h * 0.5f, hd - b_thick * 0.5f, hw * 2.0f, b_h, b_thick, 12.0f, 1.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    add_box_textured(hazard_verts, -hw + b_thick * 0.5f, b_h * 0.5f, 0.0f, b_thick, b_h, hd * 2.0f, 1.0f, 8.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    add_box_textured(hazard_verts, hw - b_thick * 0.5f, b_h * 0.5f, 0.0f, b_thick, b_h, hd * 2.0f, 1.0f, 8.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    s_vao_hazard = create_mesh_vao(hazard_verts, s_vbo_hazard);
    s_count_hazard = (int)hazard_verts.size();

    // D) Doble Cinta Transportadora Central con Rodillos Texturados
    std::vector<DioramaVertex> belt_verts;
    add_box_textured(belt_verts, 0.0f, 0.006f, -0.06f, hw * 1.75f, 0.010f, 0.07f, 1.0f, 14.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    add_box_textured(belt_verts, 0.0f, 0.006f, 0.06f, hw * 1.75f, 0.010f, 0.07f, 1.0f, 14.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    // Cinta secundaria que alimenta la máquina ensambladora
    add_box_textured(belt_verts, -0.22f, 0.006f, 0.0f, 0.07f, 0.010f, 0.22f, 1.0f, 3.0f, 1.0f, 1.0f, 1.0f, 1.0f);
    s_vao_belt = create_mesh_vao(belt_verts, s_vbo_belt);
    s_count_belt = (int)belt_verts.size();

    // E) Carga individual de Modelos 3D de factorio-3d-mod para Entidades Vivas
    load_or_create_fallback(ENTITY_ASSEMBLING_MACHINE, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/assembling-machine/static.glb", 0.18f);
    load_or_create_fallback(ENTITY_MINING_DRILL, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/electric-mining-drill/static.glb", 0.16f);
    load_or_create_fallback(ENTITY_FURNACE, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/steel-furnace/static.glb", 0.13f);
    load_or_create_fallback(ENTITY_RADAR, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/radar/static1.glb", 0.14f);
    load_or_create_fallback(ENTITY_ROBOPORT, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/roboport/static.glb", 0.18f);
    load_or_create_fallback(ENTITY_GUN_TURRET, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/gun-turret/static1.glb", 0.10f);
    load_mesh_from_glb(s_turret_top_mesh, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/gun-turret/static2.glb", 0.09f);
    load_or_create_fallback(ENTITY_CHEST, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/steel-chest/static.glb", 0.07f);
    load_or_create_fallback(ENTITY_ELECTRIC_POLE, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/medium-electric-pole/static.glb", 0.06f);
    load_or_create_fallback(ENTITY_TRANSPORT_BELT, nullptr, 0.04f);
    load_or_create_fallback(ENTITY_BITER, "/sdcard/XFactorVR/models/ENTITIES/ENEMIES/biters/biter_tier1.glb", 0.09f);
    load_or_create_fallback(ENTITY_TREE, "/sdcard/XFactorVR/models/ENTITIES/FOLIAGE/tree1/static.glb", 0.13f);
    load_or_create_fallback(ENTITY_PLAYER, "/sdcard/XFactorVR/models/ENTITIES/PLAYER/idle.glb", 0.075f);

    // F) Consola Holográfica Espacial y Quickbar 6DoF (Generals XR pattern)
    // Flotante y angulada hacia los ojos del jugador
    std::vector<DioramaVertex> ui_verts;
    float ui_w = 0.62f, ui_h = 0.28f;
    float ui_z = -hd - 0.08f;
    float ui_y = 0.18f;

    // Panel de cristal oscuro translúcido (#0e141a, alpha 0.94)
    add_box_textured(ui_verts, 0.0f, ui_y, ui_z, ui_w, ui_h, 0.005f, 1.0f, 1.0f, 0.07f, 0.10f, 0.14f, 0.94f);
    // Barra de título superior naranja Factorio
    add_box_textured(ui_verts, 0.0f, ui_y + ui_h * 0.44f, ui_z + 0.004f, ui_w * 0.95f, 0.022f, 0.002f, 1.0f, 1.0f, 1.0f, 0.58f, 0.12f, 1.0f);

    // F.1) 5 Botones de comando Factorio:
    // [0: OBSERVADOR 1:1] [1: RE-ANCLAR] [2: ENERGÍA] [3: CONSTRUIR] [4: TECNOLOGÍA]
    float btn_w = 0.108f, btn_h = 0.032f;
    for (int b = -2; b <= 2; ++b) {
        float bx = 0.0f + (float)b * (btn_w + 0.012f);
        float br = (b == -2) ? 0.85f : 0.18f;
        float bg = (b == -2) ? 0.35f : 0.52f;
        float bb = (b == -2) ? 0.20f : 0.70f;
        add_box_textured(ui_verts, bx, ui_y + 0.065f, ui_z + 0.004f, btn_w, btn_h, 0.003f, 1.0f, 1.0f, br, bg, bb, 1.0f);
    }

    // F.2) 9 Ranuras de Construcción 6DoF / Quickbar (Estilo Generals XR Command Palette):
    // [0: Ensambladora] [1: Minera] [2: Horno] [3: Torreta] [4: Radar] [5: Robopuerto] [6: Cofre] [7: Poste] [8: Demoler]
    float slot_w = 0.055f, slot_h = 0.038f;
    for (int s = 0; s < 9; ++s) {
        float sx = (float)(s - 4) * (slot_w + 0.010f);
        float sr = 0.25f, sg = 0.45f, sb = 0.65f;
        switch (s) {
            case 0: sr = 0.25f; sg = 0.48f; sb = 0.68f; break; // Ensambladora (Azul)
            case 1: sr = 0.18f; sg = 0.62f; sb = 0.58f; break; // Minera (Teal)
            case 2: sr = 0.52f; sg = 0.38f; sb = 0.30f; break; // Horno (Terracota)
            case 3: sr = 0.42f; sg = 0.46f; sb = 0.50f; break; // Torreta (Gris militar)
            case 4: sr = 0.32f; sg = 0.58f; sb = 0.78f; break; // Radar (Celeste)
            case 5: sr = 0.78f; sg = 0.48f; sb = 0.22f; break; // Robopuerto (Naranja)
            case 6: sr = 0.52f; sg = 0.56f; sb = 0.62f; break; // Cofre (Acero)
            case 7: sr = 0.56f; sg = 0.44f; sb = 0.28f; break; // Poste (Cobre/Madera)
            case 8: sr = 0.88f; sg = 0.22f; sb = 0.18f; break; // Demolición (Rojo advertencia)
        }
        add_box_textured(ui_verts, sx, ui_y - 0.055f, ui_z + 0.004f, slot_w, slot_h, 0.003f, 1.0f, 1.0f, sr, sg, sb, 1.0f);
    }

    // Barra de estado de producción / investigación activa
    add_box_textured(ui_verts, 0.0f, ui_y + 0.005f, ui_z + 0.003f, ui_w * 0.92f, 0.045f, 0.002f, 1.0f, 1.0f, 0.12f, 0.16f, 0.22f, 0.85f);
    add_box_textured(ui_verts, 0.0f, ui_y + 0.005f, ui_z + 0.005f, ui_w * 0.88f, 0.012f, 0.002f, 1.0f, 1.0f, 0.20f, 0.75f, 0.95f, 1.0f);

    s_vao_ui = create_mesh_vao(ui_verts, s_vbo_ui);
    s_count_ui = (int)ui_verts.size();

    // G) Buffer dinámico para piezas móviles animadas en tiempo real
    glGenVertexArrays(1, &s_dynamic_vao);
    glBindVertexArray(s_dynamic_vao);
    glGenBuffers(1, &s_dynamic_vbo);
    glBindBuffer(GL_ARRAY_BUFFER, s_dynamic_vbo);
    glBufferData(GL_ARRAY_BUFFER, 8192 * sizeof(DioramaVertex), nullptr, GL_DYNAMIC_DRAW);

    glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, sizeof(DioramaVertex), (void*)0);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(1, 3, GL_FLOAT, GL_FALSE, sizeof(DioramaVertex), (void*)(3 * sizeof(float)));
    glEnableVertexAttribArray(1);
    glVertexAttribPointer(2, 2, GL_FLOAT, GL_FALSE, sizeof(DioramaVertex), (void*)(6 * sizeof(float)));
    glEnableVertexAttribArray(2);
    glVertexAttribPointer(3, 4, GL_FLOAT, GL_FALSE, sizeof(DioramaVertex), (void*)(8 * sizeof(float)));
    glEnableVertexAttribArray(3);

    glBindVertexArray(0);
    glBindBuffer(GL_ARRAY_BUFFER, 0);

    LOGI("Diorama Tabletop Factorio inicializado con maqueta 3D completa estilo Generals XR");
}

static void trigger_controller_haptics(XrSession session, XrAction haptic_action, float amplitude = 0.7f, float duration_ms = 40.0f) {
    if (session == XR_NULL_HANDLE || haptic_action == XR_NULL_HANDLE) return;
    XrHapticVibration vib = {
        .type = XR_TYPE_HAPTIC_VIBRATION,
        .next = nullptr,
        .duration = (XrDuration)(duration_ms * 1000000.0f),
        .frequency = 180.0f,
        .amplitude = amplitude
    };
    XrHapticActionInfo hapticInfo = {
        .type = XR_TYPE_HAPTIC_ACTION_INFO,
        .next = nullptr,
        .action = haptic_action,
        .subactionPath = XR_NULL_PATH
    };
    xrApplyHapticFeedback(session, &hapticInfo, (XrHapticBaseHeader*)&vib);
}

// Panel con el render real del juego (definido mas abajo, en su propio
// bloque: necesita sockets y textura).
static void draw_game_panel(const float* vp_mat, const float* model_mesa, bool ojo_primario);

static void draw_tabletop_scene(const float* vp_mat,
                                float anim_time,
                                float dt,
                                const XrPosef& right_aim_pose,
                                bool right_aim_valid,
                                bool trigger_clicked,
                                bool trigger_held,
                                bool grip_clicked,
                                bool is_primary_eye,
                                XrSession session = XR_NULL_HANDLE,
                                XrAction haptic_action = XR_NULL_HANDLE) {
    if (s_program == 0) return;

    glUseProgram(s_program);
    glEnable(GL_DEPTH_TEST);
    glDepthFunc(GL_LEQUAL);
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);

    // Matriz de modelo activa (Diorama de mesa o Modo Observador a escala 1:1)
    float active_model[16];
    if (s_observer_mode) {
        float m_trans[16], m_scale[16];
        mat4_scale(m_scale, 25.0f, 25.0f, 25.0f);
        mat4_translate(m_trans, -s_observer_pos.x * 25.0f, 0.0f, -s_observer_pos.z * 25.0f);
        mat4_multiply(m_trans, m_scale, active_model);
    } else {
        memcpy(active_model, s_table_model_mat, sizeof(active_model));
    }

    float mvp_table[16];
    mat4_multiply(vp_mat, active_model, mvp_table);
    glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, active_model);
    glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, mvp_table);

    // Límites de mesa para recorte de bordes (Generals XR pattern)
    float bounds[4] = { -TABLE_HALF_W, -TABLE_HALF_D, TABLE_HALF_W, TABLE_HALF_D };
    glUniform4fv(s_board_bounds_loc, 1, bounds);
    glUniform1f(s_enable_clipping_loc, s_observer_mode ? 0.0f : 1.0f);
    glUniform1f(s_lit_loc, 1.0f);
    glUniform3f(s_emissive_loc, 0.0f, 0.0f, 0.0f);
    glUniform2f(s_uv_offset_loc, 0.0f, 0.0f);

    glActiveTexture(GL_TEXTURE0);
    glUniform1i(s_tex_loc, 0);
    glUniform1f(s_use_tex_loc, 1.0f);

    // A) Sustrato geológico del plinto (s_tex_soil)
    if (!s_observer_mode) {
        glBindTexture(GL_TEXTURE_2D, s_tex_soil);
        glBindVertexArray(s_vao_soil);
        glDrawArrays(GL_TRIANGLES, 0, s_count_soil);
    }

    // B) Suelo de concreto industrial (s_tex_concrete)
    glBindTexture(GL_TEXTURE_2D, s_tex_concrete);
    glBindVertexArray(s_vao_concrete);
    glDrawArrays(GL_TRIANGLES, 0, s_count_concrete);

    // C) Franjas perimetrales de advertencia (s_tex_hazard)
    if (!s_observer_mode) {
        glBindTexture(GL_TEXTURE_2D, s_tex_hazard);
        glBindVertexArray(s_vao_hazard);
        glDrawArrays(GL_TRIANGLES, 0, s_count_hazard);
    }

    // D) Cinta transportadora con rodillos animados en flujo continuo (s_tex_belt)
    glBindTexture(GL_TEXTURE_2D, s_tex_belt);
    glBindVertexArray(s_vao_belt);
    float belt_scroll = fmodf(anim_time * 0.35f, 1.0f);
    glUniform2f(s_uv_offset_loc, 0.0f, belt_scroll);
    glDrawArrays(GL_TRIANGLES, 0, s_count_belt);
    glUniform2f(s_uv_offset_loc, 0.0f, 0.0f);

    // E) RENDERIZADO DINÁMICO DE ENTIDADES VIVAS (FASE 1: SIMULACIÓN EN VIVO)
    LiveEntity live_entities[128];
    int entity_count = 0;
    if (s_get_live_entities) {
        s_get_live_entities(live_entities, 128, &entity_count);
    }

    if (is_primary_eye && s_update_simulation) {
        s_update_simulation(dt);
    }

    glUniform1f(s_use_tex_loc, 0.0f); // Usar colores vertex-baked originales de factorio-3d-mod
    glUniform1f(s_lit_loc, 1.0f);
    glUniform3f(s_emissive_loc, 0.0f, 0.0f, 0.0f);

    std::vector<LiveEntity> electric_poles;
    for (int p = 0; p < XFACTORVR_MAX_ATLAS_PAGES; ++p) {
        s_sprite_batches[p].verts.clear();
    }

    for (int i = 0; i < entity_count; ++i) {
        const LiveEntity& ent = live_entities[i];
        if (ent.entity_type <= 0 || ent.entity_type >= ENTITY_TYPE_COUNT) continue;

        if (ent.entity_type == ENTITY_ELECTRIC_POLE) {
            electric_poles.push_back(ent);
        }

        // Si el juego tiene sprite para este tipo, se dibuja el sprite REAL
        // tumbado sobre la mesa en lugar de una primitiva inventada.
        if (ent.sprite_id != 0 && s_get_sprite_frame && ensure_atlas_texture()) {
            float rect[10];
            if (s_get_sprite_frame(ent.sprite_id, ent.anim_frame, ent.orientation, rect)) {
                int page = (int)rect[9];
                if (page >= 0 && page < XFACTORVR_MAX_ATLAS_PAGES) {
                    float mx = (ent.tile_x - s_view_pan_x) * 0.04f;
                    float mz = (ent.tile_y - s_view_pan_y) * 0.04f;
                    // El sprite mide frame_width px a escala s; en el juego una
                    // casilla son 32 px y aqui una casilla son 0.04 m.
                    float tiles = rect[4] * rect[8] / 32.0f;
                    float half = tiles * 0.04f * 0.5f;
                    float pw = (float)s_atlas_page_w[page];
                    float ph = (float)s_atlas_page_h[page];
                    float u0 = rect[0] / pw;
                    float v0 = rect[1] / ph;
                    float u1 = (rect[0] + rect[2]) / pw;
                    float v1 = (rect[1] + rect[3]) / ph;
                    add_sprite_quad(s_sprite_batches[page].verts, mx, mz, 0.0012f,
                                    half, half, u0, v0, u1, v1,
                                    1.0f, 1.0f, 1.0f, 1.0f);
                    continue;  // sprite real: no se dibuja la malla primitiva
                }
            }
        }

        const ModelMesh& mesh = s_entity_meshes[ent.entity_type];
        if (mesh.vao == 0 || mesh.vertex_count == 0) continue;

        float mx = (ent.tile_x - s_view_pan_x) * 0.04f;
        float mz = (ent.tile_y - s_view_pan_y) * 0.04f;

        float ent_trans[16], ent_rot[16], ent_local[16], ent_world[16], ent_mvp[16];
        mat4_rotate_y(ent_rot, ent.orientation);
        mat4_translate(ent_trans, mx, 0.0f, mz);
        mat4_multiply(ent_trans, ent_rot, ent_local);
        mat4_multiply(active_model, ent_local, ent_world);
        mat4_multiply(vp_mat, ent_world, ent_mvp);

        glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, ent_world);
        glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, ent_mvp);

        glBindVertexArray(mesh.vao);
        glDrawArrays(GL_TRIANGLES, 0, mesh.vertex_count);

        // Si es torreta, dibujar el cañón / torreta superior rotatoria
        if (ent.entity_type == ENTITY_GUN_TURRET && s_turret_top_mesh.vao != 0) {
            float top_trans[16], top_rot[16], top_local[16], top_world[16], top_mvp[16];
            float turret_yaw = ent.orientation + sinf(anim_time * 0.8f + (float)i) * 0.65f;
            mat4_rotate_y(top_rot, turret_yaw);
            mat4_translate(top_trans, mx, 0.045f, mz);
            mat4_multiply(top_trans, top_rot, top_local);
            mat4_multiply(active_model, top_local, top_world);
            mat4_multiply(vp_mat, top_world, top_mvp);

            glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, top_world);
            glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, top_mvp);

            glBindVertexArray(s_turret_top_mesh.vao);
            glDrawArrays(GL_TRIANGLES, 0, s_turret_top_mesh.vertex_count);
        }

        // Emisión de partículas periódicas por máquina activa (Fase 3)
        if (is_primary_eye) {
            if (ent.entity_type == ENTITY_ASSEMBLING_MACHINE && (rand() % 10 == 0)) {
                spawn_particle(mx - 0.045f, 0.16f, mz - 0.045f,
                               ((rand() % 100) - 50) * 0.0002f, 0.035f, ((rand() % 100) - 50) * 0.0002f,
                               0.85f, 0.88f, 0.92f, 0.50f, 0.016f, 1.2f);
            } else if (ent.entity_type == ENTITY_FURNACE && ent.status == 1) {
                if (rand() % 4 == 0) {
                    spawn_particle(mx, 0.14f, mz,
                                   ((rand() % 100) - 50) * 0.0003f, 0.040f, ((rand() % 100) - 50) * 0.0003f,
                                   0.35f, 0.35f, 0.35f, 0.75f, 0.016f, 1.2f);
                }
                if (rand() % 5 == 0) {
                    spawn_particle(mx, 0.035f, mz + 0.038f,
                                   ((rand() % 100) - 50) * 0.0004f, 0.020f, ((rand() % 100) - 50) * 0.0004f,
                                   1.0f, 0.85f, 0.10f, 0.95f, 0.006f, 0.5f);
                }
            } else if (ent.entity_type == ENTITY_MINING_DRILL && ent.status == 1) {
                if (rand() % 4 == 0) {
                    spawn_particle(mx, 0.13f, mz - 0.025f,
                                   0.0f, 0.028f, 0.0f,
                                   0.45f, 0.45f, 0.45f, 0.60f, 0.014f, 0.9f);
                }
                if (rand() % 3 == 0) {
                    spawn_particle(mx, 0.015f, mz,
                                   ((rand() % 100) - 50) * 0.0005f, 0.015f, ((rand() % 100) - 50) * 0.0005f,
                                   0.75f, 0.65f, 0.35f, 0.80f, 0.008f, 0.4f);
                }
            }
        }
    }

    // Suelo real del mapa: un quad por casilla con la textura del terreno.
    // Se reconstruye cada pocos fotogramas (el terreno no cambia cada frame)
    // y se pinta antes que las entidades, para que quede debajo.
    if (is_primary_eye) {
        s_terrain_frame = (s_terrain_frame + 1) % 30;
        if (s_terrain_frame == 0) {
            build_terrain_mesh(s_view_pan_x, s_view_pan_y);
        }
    }
    for (int p = 0; p < s_terrain_page_count; ++p) {
        draw_sprite_batch(s_terrain_batches[p], (uint32_t)p, active_model, vp_mat,
                          s_tex_terrain);
    }

    // Lote de sprites reales de Factorio, tumbados sobre la mesa. Cada pagina
    // del atlas es una textura y se pinta por separado.
    for (int p = 0; p < s_atlas_page_count; ++p) {
        draw_sprite_batch(s_sprite_batches[p], (uint32_t)p, active_model, vp_mat);
    }

    // F) Consola UI Holográfica Espacial (Generals XR pattern)
    float hd = TABLE_HALF_D;
    float ui_z = -hd - 0.08f;
    float ui_y = 0.18f;

    if (!s_observer_mode) {
        float ui_mvp[16];
        mat4_multiply(vp_mat, active_model, ui_mvp);
        glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, active_model);
        glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, ui_mvp);

        glUniform1f(s_enable_clipping_loc, 0.0f); // Sin recorte para el panel flotante
        glUniform1f(s_use_tex_loc, 0.0f);
        glUniform1f(s_lit_loc, 0.35f);
        glBindVertexArray(s_vao_ui);
        glDrawArrays(GL_TRIANGLES, 0, s_count_ui);
        glUniform1f(s_enable_clipping_loc, 1.0f); // Restaurar recorte de la mesa física
    }

    // G) GEOMETRÍA DINÁMICA DE LA FÁBRICA Y PIEZAS MÓVILES (FASE 3)
    std::vector<DioramaVertex> dyn_verts;

    // 0. Yacimientos reales de recursos minerales en la superficie de la mesa (Hierro, Cobre, Carbón, Piedra)
    ResourcePatchFfi resource_patches[16];
    uint32_t patch_count = 0;
    if (s_get_resource_patches && s_get_resource_patches(resource_patches, 16, &patch_count)) {
        for (uint32_t p = 0; p < patch_count; ++p) {
            const ResourcePatchFfi& patch = resource_patches[p];
            float px = (patch.tile_x - s_view_pan_x) * 0.04f;
            float pz = (patch.tile_y - s_view_pan_y) * 0.04f;
            float pr = patch.radius * 0.04f;

            float col_r = 0.5f, col_g = 0.5f, col_b = 0.5f;
            switch (patch.item_type) {
                case 1: col_r = 0.28f; col_g = 0.44f; col_b = 0.62f; break; // Iron Ore (Azulado)
                case 2: col_r = 0.78f; col_g = 0.42f; col_b = 0.20f; break; // Copper Ore (Cobrizo)
                case 3: col_r = 0.11f; col_g = 0.11f; col_b = 0.12f; break; // Coal (Carbón oscuro)
                case 4: col_r = 0.65f; col_g = 0.62f; col_b = 0.55f; break; // Stone (Gris piedra)
            }
            add_cylinder_textured(dyn_verts, px, 0.003f, pz, pr, 0.003f, col_r * 0.65f, col_g * 0.65f, col_b * 0.65f, 0.95f, 16);

            for (int k = 0; k < 7; ++k) {
                float angle = (float)k * (6.2831853f / 7.0f) + (float)patch.item_type;
                float dist = pr * 0.58f;
                float ox = px + cosf(angle) * dist;
                float oz = pz + sinf(angle) * dist;
                float n_size = 0.016f + ((k % 3) * 0.004f);
                add_box_textured(dyn_verts, ox, 0.006f, oz, n_size, 0.007f, n_size, 1.0f, 1.0f, col_r, col_g, col_b, 1.0f);
            }
        }
    }

    // 1. Cableado catenario entre postes eléctricos existentes
    for (size_t p1 = 0; p1 < electric_poles.size(); ++p1) {
        for (size_t p2 = p1 + 1; p2 < electric_poles.size(); ++p2) {
            float dx = electric_poles[p1].tile_x - electric_poles[p2].tile_x;
            float dy = electric_poles[p1].tile_y - electric_poles[p2].tile_y;
            float dist = sqrtf(dx * dx + dy * dy);
            if (dist < 10.0f) { // Alcance de conexión del poste mediano (10 tiles = 0.40m)
                float x0 = (electric_poles[p1].tile_x - s_view_pan_x) * 0.04f;
                float z0 = (electric_poles[p1].tile_y - s_view_pan_y) * 0.04f;
                float x1 = (electric_poles[p2].tile_x - s_view_pan_x) * 0.04f;
                float z1 = (electric_poles[p2].tile_y - s_view_pan_y) * 0.04f;
                add_catenary_cable(dyn_verts, x0, 0.22f, z0, x1, 0.22f, z1, 0.035f, 8, 0.20f, 0.22f, 0.24f, 1.0f);
            }
        }
    }

    // 2. Piezas móviles animadas ancladas a entidades vivas activas
    for (int i = 0; i < entity_count; ++i) {
        const LiveEntity& ent = live_entities[i];
        float mx = (ent.tile_x - s_view_pan_x) * 0.04f;
        float mz = (ent.tile_y - s_view_pan_y) * 0.04f;

        if (ent.entity_type == ENTITY_RADAR) {
            // Antena parabólica giratoria continua 360°
            float rad_ang = anim_time * 1.5f;
            float dish_dx = sinf(rad_ang) * 0.04f;
            float dish_dz = cosf(rad_ang) * 0.04f;
            add_box_textured(dyn_verts, mx, 0.13f, mz, 0.08f, 0.018f, 0.025f, 1.0f, 1.0f, 0.82f, 0.85f, 0.88f, 1.0f);
            add_cylinder_textured(dyn_verts, mx + dish_dx, 0.13f, mz + dish_dz, 0.035f, 0.010f, 0.92f, 0.94f, 0.96f, 1.0f, 10);
        } else if (ent.entity_type == ENTITY_ROBOPORT) {
            // Drones logísticos en patrulla aérea orbital
            for (int d = 0; d < 2; ++d) {
                float d_ang = anim_time * 1.8f + (float)d * 3.14159f;
                float drone_x = mx + sinf(d_ang) * 0.08f;
                float drone_z = mz + sinf(d_ang * 2.0f) * 0.05f;
                float drone_y = 0.18f + cosf(d_ang) * 0.02f;
                add_box_textured(dyn_verts, drone_x, drone_y, drone_z, 0.024f, 0.010f, 0.024f, 1.0f, 1.0f, 0.90f, 0.92f, 0.95f, 1.0f);
                add_cylinder_textured(dyn_verts, drone_x - 0.014f, drone_y + 0.006f, drone_z - 0.014f, 0.008f, 0.002f, 0.2f, 0.2f, 0.2f, 0.8f, 6);
                add_cylinder_textured(dyn_verts, drone_x + 0.014f, drone_y + 0.006f, drone_z - 0.014f, 0.008f, 0.002f, 0.2f, 0.2f, 0.2f, 0.8f, 6);
                add_cylinder_textured(dyn_verts, drone_x - 0.014f, drone_y + 0.006f, drone_z + 0.014f, 0.008f, 0.002f, 0.2f, 0.2f, 0.2f, 0.8f, 6);
                add_cylinder_textured(dyn_verts, drone_x + 0.014f, drone_y + 0.006f, drone_z + 0.014f, 0.008f, 0.002f, 0.2f, 0.2f, 0.2f, 0.8f, 6);
            }
        } else if (ent.entity_type == ENTITY_MINING_DRILL) {
            // Vástago y broca reciprocante giratoria
            float drill_y = 0.045f + sinf(anim_time * 8.0f) * 0.012f;
            add_cylinder_textured(dyn_verts, mx, drill_y, mz, 0.024f, 0.07f, 0.55f, 0.60f, 0.65f, 1.0f, 8);
        } else if (ent.entity_type == ENTITY_FURNACE) {
            // Fuego del hogar incandescente cuando está fundiendo (status == 1)
            if (ent.status == 1) {
                float flicker = 0.85f + 0.15f * sinf(anim_time * 25.0f + (float)i * 3.1f);
                add_box_textured(dyn_verts, mx, 0.032f, mz + 0.038f, 0.040f * flicker, 0.028f * flicker, 0.015f, 1.0f, 1.0f, 1.0f, 0.55f * flicker, 0.05f, 1.0f);
            }
            // Indicador holográfico de planchas fundidas listas para recoger
            float m_fuel = 0.0f, m_prog = 0.0f;
            uint32_t m_in = 0, m_out = 0;
            if (s_get_machine_info && s_get_machine_info(ent.tile_x, ent.tile_y, &m_fuel, &m_prog, &m_in, &m_out) && m_out > 0) {
                float bob = sinf(anim_time * 4.5f + (float)i) * 0.006f;
                add_box_textured(dyn_verts, mx, 0.14f + bob, mz, 0.028f, 0.005f, 0.028f, 1.0f, 1.0f, 1.0f, 0.88f, 0.20f, 0.95f);
                add_box_textured(dyn_verts, mx, 0.14f + bob, mz, 0.034f, 0.002f, 0.034f, 1.0f, 1.0f, 1.0f, 1.0f, 0.40f, 0.60f);
            }
        } else if (ent.entity_type == ENTITY_ASSEMBLING_MACHINE) {
            // Baliza titilante en chimenea
            float beacon_g = 0.58f + sinf(anim_time * 5.0f) * 0.38f;
            add_cylinder_textured(dyn_verts, mx + 0.065f, 0.12f, mz + 0.065f, 0.008f, 0.016f, 1.0f, beacon_g, 0.1f, 1.0f, 6);
        } else if (ent.entity_type == ENTITY_TRANSPORT_BELT || ent.entity_type == ENTITY_INSERTER) {
            // Piezas que circulan por la fabrica: se dibujan traveling sobre la
            // cinta o en la punta del brazo del insertador, para que se vea
            // como avanza el material en tiempo real.
            uint32_t carried_id = 0, carried_count = 0;
            bool has_carried = s_get_carried_item &&
                s_get_carried_item(ent.tile_x, ent.tile_y, &carried_id, &carried_count) &&
                carried_count > 0;

            if (has_carried) {
                float ir = 0.85f, ig = 0.85f, ib = 0.85f;
                item_color(carried_id, ir, ig, ib);

                float item_x = mx, item_z = mz, item_y = 0.022f;

                if (ent.entity_type == ENTITY_TRANSPORT_BELT) {
                    // La pieza avanza a lo largo de la cinta segun su direccion.
                    float bdx = sinf(ent.orientation);
                    float bdz = -cosf(ent.orientation);
                    float travel = (ent.animation_progress - 0.5f) * 0.04f;
                    item_x = mx + bdx * travel;
                    item_z = mz + bdz * travel;
                } else {
                    // El insertador sostiene la pieza en la punta del brazo.
                    // Con Factorio real se usa el angulo vivo del brazo.
                    float arm_a = ent.arm_angle;
                    if (arm_a == 0.0f) {
                        arm_a = ent.orientation + sinf(anim_time * 2.2f) * 0.82f;
                    }
                    item_x = mx + sinf(arm_a) * 0.034f;
                    item_z = mz - cosf(arm_a) * 0.034f;
                    item_y = 0.030f;
                }

                add_box_textured(dyn_verts, item_x, item_y, item_z, 0.008f, 0.008f, 0.008f, 1.0f, 1.0f, ir, ig, ib, 1.0f);
            }
        }
    }

    // 3. Inserter articulado estándar (brazo con cinemática oscilante)
    float ins1_yaw = sinf(anim_time * 2.2f) * 0.82f;
    add_cylinder_textured(dyn_verts, -0.08f, 0.02f, -0.13f, 0.025f, 0.04f, 0.95f, 0.68f, 0.12f, 1.0f, 8);
    float elb1_x = -0.08f + sinf(ins1_yaw) * 0.06f;
    float elb1_z = -0.13f + cosf(ins1_yaw) * 0.06f;
    add_box_textured(dyn_verts, (-0.08f + elb1_x) * 0.5f, 0.04f, (-0.13f + elb1_z) * 0.5f, 0.012f, 0.012f, 0.07f, 1.0f, 1.0f, 0.28f, 0.32f, 0.36f, 1.0f);

    // 4. Ítems transportados sobre las cintas
    float item_span = TABLE_HALF_W * 1.55f;
    for (int k = 0; k < 7; ++k) {
        float offset = fmodf(anim_time * 0.16f + (float)k * (item_span / 7.0f), item_span) - item_span * 0.5f;
        add_box_textured(dyn_verts, offset, 0.016f, -0.06f, 0.030f, 0.005f, 0.030f, 1.0f, 1.0f, 0.68f, 0.74f, 0.80f, 1.0f);
        add_box_textured(dyn_verts, -offset, 0.016f, 0.06f, 0.026f, 0.004f, 0.026f, 1.0f, 1.0f, 0.16f, 0.82f, 0.35f, 1.0f);
    }

    // 5. Partículas activas (Fase 3: Humo, Vapor, Polvo y Chispas)
    for (size_t p = 0; p < s_particles.size(); ) {
        Particle& part = s_particles[p];
        if (is_primary_eye) {
            part.life -= dt;
            part.x += part.vx * dt;
            part.y += part.vy * dt;
            part.z += part.vz * dt;
        }
        if (part.life <= 0.0f) {
            s_particles[p] = s_particles.back();
            s_particles.pop_back();
            continue;
        }
        float alpha = part.a * (part.life / part.max_life);
        add_box_textured(dyn_verts, part.x, part.y, part.z, part.size, part.size, part.size,
                         1.0f, 1.0f, part.r, part.g, part.b, alpha);
        ++p;
    }

    // 6. Consola UI Holográfica Espacial: Renderizado dinámico según pestaña activa
    if (!s_observer_mode) {
        if (s_active_ui_tab == 0) {
            // Pestaña 0: Construcción / Quickbar
            int selected_slot = -1;
            const int slot_to_entity[9] = {
                ENTITY_ASSEMBLING_MACHINE, ENTITY_MINING_DRILL, ENTITY_FURNACE,
                ENTITY_GUN_TURRET, ENTITY_RADAR, ENTITY_ROBOPORT,
                ENTITY_CHEST, ENTITY_ELECTRIC_POLE, ENTITY_NONE
            };
            for (int s = 0; s < 9; ++s) {
                if (slot_to_entity[s] == s_selected_build_tool) {
                    selected_slot = s;
                    break;
                }
            }
            float slot_w = 0.055f, slot_h = 0.038f;
            if (selected_slot >= 0) {
                float sx = (float)(selected_slot - 4) * (slot_w + 0.010f);
                // Marco de selección dorado brillante (#ffd700)
                add_box_textured(dyn_verts, sx, ui_y - 0.055f, ui_z + 0.007f, slot_w + 0.006f, slot_h + 0.006f, 0.002f, 1.0f, 1.0f, 1.0f, 0.84f, 0.10f, 0.95f);
            }
            if (s_hovered_quickbar_slot >= 0 && s_hovered_quickbar_slot != selected_slot) {
                float hx = (float)(s_hovered_quickbar_slot - 4) * (slot_w + 0.010f);
                add_box_textured(dyn_verts, hx, ui_y - 0.055f, ui_z + 0.006f, slot_w + 0.004f, slot_h + 0.004f, 0.002f, 1.0f, 1.0f, 0.20f, 0.95f, 1.00f, 0.85f);
            }
        } else if (s_active_ui_tab == 1) {
            // Pestaña 1: Inventario del jugador (Factorio Player Inventory)
            PlayerItemFfi inv_items[32];
            uint32_t inv_count = 0;
            if (s_get_player_inventory && s_get_player_inventory(inv_items, 32, &inv_count)) {
                float slot_w = 0.046f, slot_h = 0.032f;
                uint32_t display_count = std::min(inv_count, 14u);
                for (uint32_t i = 0; i < display_count; ++i) {
                    int col = (int)i % 7;
                    int row = (int)i / 7;
                    float ix = (float)(col - 3) * (slot_w + 0.008f);
                    float iy = ui_y - 0.042f - (float)row * (slot_h + 0.008f);

                    float ir = 0.5f, ig = 0.5f, ib = 0.5f;
                    switch (inv_items[i].item_id) {
                        case 1: ir = 0.28f; ig = 0.44f; ib = 0.62f; break; // Iron Ore
                        case 2: ir = 0.78f; ig = 0.42f; ib = 0.20f; break; // Copper Ore
                        case 3: ir = 0.12f; ig = 0.12f; ib = 0.14f; break; // Coal
                        case 4: ir = 0.65f; ig = 0.62f; ib = 0.55f; break; // Stone
                        case 5: ir = 0.82f; ig = 0.85f; ib = 0.90f; break; // Iron Plate
                        case 6: ir = 0.88f; ig = 0.52f; ib = 0.25f; break; // Copper Plate
                        case 7: ir = 0.95f; ig = 0.78f; ib = 0.20f; break; // Iron Gear
                        case 8: ir = 0.20f; ig = 0.88f; ib = 0.35f; break; // Electronic Circuit
                        case 9: ir = 0.25f; ig = 0.48f; ib = 0.68f; break; // Assembling Machine
                        case 10: ir = 0.18f; ig = 0.62f; ib = 0.58f; break; // Mining Drill
                        case 11: ir = 0.52f; ig = 0.38f; ib = 0.30f; break; // Furnace
                        case 12: ir = 0.92f; ig = 0.82f; ib = 0.15f; break; // Transport Belt
                        case 13: ir = 0.95f; ig = 0.68f; ib = 0.12f; break; // Inserter
                        case 14: ir = 0.56f; ig = 0.44f; ib = 0.28f; break; // Electric Pole
                        default: ir = 0.45f; ig = 0.50f; ib = 0.55f; break;
                    }
                    add_box_textured(dyn_verts, ix, iy, ui_z + 0.005f, slot_w, slot_h, 0.002f, 1.0f, 1.0f, ir, ig, ib, 0.95f);
                    float fill = std::clamp((float)inv_items[i].count / 50.0f, 0.15f, 1.0f);
                    add_box_textured(dyn_verts, ix, iy - slot_h * 0.34f, ui_z + 0.007f, slot_w * fill, 0.004f, 0.002f, 1.0f, 1.0f, 0.2f, 0.95f, 0.4f, 1.0f);
                }
            }
        } else if (s_active_ui_tab == 2) {
            // Pestaña 2: Fabricación / Crafteo de recetas (7 recetas auténticas)
            float recipe_w = 0.088f, recipe_h = 0.046f;
            for (int r = 0; r < CRAFT_RECIPE_COUNT; ++r) {
                float rx_btn = (float)(r - (CRAFT_RECIPE_COUNT - 1) / 2) * (recipe_w + 0.008f);
                float ry_btn = ui_y - 0.055f;

                float rr = 0.2f, rg = 0.5f, rb = 0.7f;
                switch (r) {
                    case 0: rr = 0.65f; rg = 0.55f; rb = 0.50f; break; // Horno de Piedra (5 Piedra)
                    case 1: rr = 0.95f; rg = 0.78f; rb = 0.20f; break; // Engranaje (2 Planchas Hierro)
                    case 2: rr = 0.20f; rg = 0.88f; rb = 0.35f; break; // Circuito Verde (1 Hierro + 3 Cobre)
                    case 3: rr = 0.92f; rg = 0.82f; rb = 0.15f; break; // Cinta Transportadora (1 Hierro + 1 Engranaje)
                    case 4: rr = 0.18f; rg = 0.62f; rb = 0.58f; break; // Minera Eléctrica/Vapor (9 Hierro + 5 Engr + 5 Piedra)
                    case 5: rr = 0.96f; rg = 0.72f; rb = 0.12f; break; // Insertador Robótico (1 Hierro + 1 Engr + 1 Circuito)
                    case 6: rr = 0.25f; rg = 0.45f; rb = 0.62f; break; // Ensambladora (9 Hierro + 5 Engr + 3 Circuitos)
                }
                add_box_textured(dyn_verts, rx_btn, ry_btn, ui_z + 0.005f, recipe_w, recipe_h, 0.002f, 1.0f, 1.0f, rr * 0.85f, rg * 0.85f, rb * 0.85f, 0.95f);
                if (s_hovered_recipe == r) {
                    add_box_textured(dyn_verts, rx_btn, ry_btn, ui_z + 0.008f, recipe_w + 0.006f, recipe_h + 0.006f, 0.002f, 1.0f, 1.0f, 1.0f, 1.0f, 0.3f, 1.0f);
                }
            }
        }
    }

    // ========================================================================
    // H) RAYO LÁSER 6DoF, RETÍCULO Y CONSTRUCCIÓN / MINADO (FASE 2)
    // ========================================================================
    if (right_aim_valid) {
        float rx = right_aim_pose.position.x;
        float ry = right_aim_pose.position.y;
        float rz = right_aim_pose.position.z;

        float qx = right_aim_pose.orientation.x;
        float qy = right_aim_pose.orientation.y;
        float qz = right_aim_pose.orientation.z;
        float qw = right_aim_pose.orientation.w;
        float dx = -2.0f * (qx * qz + qw * qy);
        float dy = -2.0f * (qy * qz - qw * qx);
        float dz = -(1.0f - 2.0f * (qx * qx + qy * qy));

        float hit_stage_x = 0.0f, hit_stage_z = 0.0f;
        float loc_x = 0.0f, loc_z = 0.0f;
        bool has_table_hit = false;
        bool has_ui_hit = false;
        float ray_len = 1.6f;

        // 1. Raycast contra el plano físico de la mesa en STAGE (Y = s_table_pos.y)
        if (fabsf(dy) > 1e-4f) {
            float t = (s_table_pos.y - ry) / dy;
            if (t > 0.0f && t < 4.0f) {
                float cand_x = rx + dx * t;
                float cand_z = rz + dz * t;

                float rel_x = cand_x - s_table_pos.x;
                float rel_z = cand_z - s_table_pos.z;
                float cos_y = cosf(-s_table_yaw);
                float sin_y = sinf(-s_table_yaw);
                loc_x = (rel_x * cos_y - rel_z * sin_y) / s_table_scale;
                loc_z = (rel_x * sin_y + rel_z * cos_y) / s_table_scale;

                if (fabsf(loc_x) <= TABLE_HALF_W && fabsf(loc_z) <= TABLE_HALF_D) {
                    has_table_hit = true;
                    ray_len = t;
                    hit_stage_x = cand_x;
                    hit_stage_z = cand_z;
                }
            }
        }

        // 2. Raycast contra la Consola UI Holográfica y Quickbar
        s_hovered_ui_btn = -1;
        s_hovered_quickbar_slot = -1;
        float ui_plane_z = -TABLE_HALF_D - 0.08f;
        float ui_world_z = s_table_pos.z + (ui_plane_z * cosf(s_table_yaw));
        float t_ui = (ui_world_z - rz) / (dz != 0 ? dz : 1e-4f);
        if (t_ui > 0.0f && t_ui < ray_len) {
            float ui_hit_x = rx + dx * t_ui;
            float ui_hit_y = ry + dy * t_ui;
            float rel_x = ui_hit_x - s_table_pos.x;
            float rel_y = ui_hit_y - (s_table_pos.y + 0.18f);

            if (fabsf(rel_x) < 0.31f && fabsf(rel_y) < 0.14f) {
                has_ui_hit = true;
                ray_len = t_ui;

                // Botones de comando superiores (rel_y > 0.02)
                if (rel_y > 0.02f) {
                    float btn_w = 0.108f;
                    for (int b = -2; b <= 2; ++b) {
                        float bx = (float)b * (btn_w + 0.012f);
                        if (fabsf(rel_x - bx) < btn_w * 0.5f && fabsf(rel_y - 0.065f) < 0.018f) {
                            s_hovered_ui_btn = b + 2; // 0..4
                            break;
                        }
                    }

                    if (is_primary_eye && trigger_clicked && s_hovered_ui_btn >= 0) {
                        trigger_controller_haptics(session, haptic_action, 0.45f, 25.0f);
                        play_spatial_sound(SFX_CLICK, ui_hit_x, ui_hit_y, ui_world_z);
                        if (s_hovered_ui_btn == 0) {
                            s_observer_mode = !s_observer_mode;
                            LOGI("Modo cambiado: %s", s_observer_mode ? "OBSERVADOR 1:1" : "TABLETOP DIORAMA");
                        } else if (s_hovered_ui_btn == 1) {
                            s_table_placed = false;
                            LOGI("Re-anclando mesa...");
                        } else {
                            s_active_ui_tab = s_hovered_ui_btn - 2;
                        }
                    }
                }
                // Ranuras de Quickbar o Recetas de Crafteo (rel_y <= 0.0)
                else if (rel_y <= 0.0f && rel_y >= -0.10f) {
                    if (s_active_ui_tab == 2) {
                        // Modo Crafteo: 7 botones de recetas
                        float recipe_w = 0.088f;
                        s_hovered_recipe = -1;
                        for (int r = 0; r < CRAFT_RECIPE_COUNT; ++r) {
                            float rx_btn = (float)(r - (CRAFT_RECIPE_COUNT - 1) / 2) * (recipe_w + 0.008f);
                            if (fabsf(rel_x - rx_btn) < recipe_w * 0.5f && fabsf(rel_y - (-0.055f)) < 0.025f) {
                                s_hovered_recipe = r;
                                break;
                            }
                        }

                        if (is_primary_eye && trigger_clicked && s_hovered_recipe >= 0) {
                            // Horno(11), Engranaje(7), Circuito(8), Cinta(12), Minera(10),
                            // Insertador(13), Ensambladora(9)
                            const uint32_t recipe_item_ids[CRAFT_RECIPE_COUNT] = { 11, 7, 8, 12, 10, 13, 9 };
                            if (s_craft_item) {
                                bool crafted = s_craft_item(recipe_item_ids[s_hovered_recipe], 1);
                                if (crafted) {
                                    LOGI("Crafteado item_id=%d con éxito", recipe_item_ids[s_hovered_recipe]);
                                    trigger_controller_haptics(session, haptic_action, 0.8f, 50.0f);
                                    play_spatial_sound(SFX_CRAFT, ui_hit_x, ui_hit_y, ui_world_z);
                                    for (int k = 0; k < 18; ++k) {
                                        spawn_particle(ui_hit_x, ui_hit_y, ui_world_z,
                                                       ((rand() % 100) - 50) * 0.001f, 0.02f + (rand() % 40) * 0.0005f, ((rand() % 100) - 50) * 0.001f,
                                                       1.0f, 0.88f, 0.20f, 0.95f, 0.009f, 0.6f);
                                    }
                                }
                            }
                        }
                    } else if (s_active_ui_tab == 0) {
                        // Modo Quickbar: 9 ranuras de construcción
                        float slot_w = 0.055f;
                        for (int s = 0; s < 9; ++s) {
                            float sx = (float)(s - 4) * (slot_w + 0.010f);
                            if (fabsf(rel_x - sx) < slot_w * 0.5f && fabsf(rel_y - (-0.055f)) < 0.022f) {
                                s_hovered_quickbar_slot = s;
                                break;
                            }
                        }

                        if (is_primary_eye && trigger_clicked && s_hovered_quickbar_slot >= 0) {
                            const int slot_to_entity[9] = {
                                ENTITY_ASSEMBLING_MACHINE, ENTITY_MINING_DRILL, ENTITY_FURNACE,
                                ENTITY_GUN_TURRET, ENTITY_RADAR, ENTITY_ROBOPORT,
                                ENTITY_CHEST, ENTITY_ELECTRIC_POLE, ENTITY_NONE
                            };
                            s_selected_build_tool = slot_to_entity[s_hovered_quickbar_slot];
                            LOGI("Quickbar: Herramienta seleccionada = %d (slot %d)", s_selected_build_tool, s_hovered_quickbar_slot);
                            trigger_controller_haptics(session, haptic_action, 0.5f, 30.0f);
                            play_spatial_sound(SFX_CLICK, ui_hit_x, ui_hit_y, ui_world_z);
                            for (int k = 0; k < 12; ++k) {
                                spawn_particle(ui_hit_x, ui_hit_y, ui_world_z,
                                               ((rand() % 100) - 50) * 0.001f, ((rand() % 100) - 50) * 0.001f, 0.02f,
                                               0.20f, 0.95f, 1.0f, 0.9f, 0.008f, 0.4f);
                            }
                        }
                    }
                }
            }
        }

        // 3. Rejilla Magnética (Grid Snapping) y Ghost Preview 6DoF sobre la mesa (Fase 2)
        if (has_table_hit && !has_ui_hit) {
            float hover_tile_x = roundf(loc_x / 0.04f) + s_view_pan_x;
            float hover_tile_y = roundf(loc_z / 0.04f) + s_view_pan_y;
            // El limite es relativo al centro de la vista: con el clamp absoluto
            // la ventana quedaba anclada al area inicial y era imposible explorar
            // la fabrica mas alla de las casillas de arranque.
            hover_tile_x = std::clamp(hover_tile_x, s_view_pan_x - 14.0f, s_view_pan_x + 14.0f);
            hover_tile_y = std::clamp(hover_tile_y, s_view_pan_y - 9.0f, s_view_pan_y + 9.0f);

            bool is_occupied = false;
            for (int i = 0; i < entity_count; ++i) {
                float dx = fabsf(live_entities[i].tile_x - hover_tile_x);
                float dy = fabsf(live_entities[i].tile_y - hover_tile_y);
                float thresh = (live_entities[i].entity_type == ENTITY_ELECTRIC_POLE || 
                                live_entities[i].entity_type == ENTITY_CHEST) ? 0.8f : 1.3f;
                if (dx < thresh && dy < thresh) {
                    is_occupied = true;
                    break;
                }
            }

            // Comprobar si la casilla está sobre una veta de recurso
            int hovered_patch_type = 0;
            for (uint32_t p = 0; p < patch_count; ++p) {
                float pdx = hover_tile_x - resource_patches[p].tile_x;
                float pdy = hover_tile_y - resource_patches[p].tile_y;
                if (pdx * pdx + pdy * pdy <= resource_patches[p].radius * resource_patches[p].radius && resource_patches[p].remaining > 0) {
                    hovered_patch_type = resource_patches[p].item_type;
                    break;
                }
            }

            // Cuadrado de la celda de rejilla magnética 4cm x 4cm
            float snap_mx = (hover_tile_x - s_view_pan_x) * 0.04f;
            float snap_mz = (hover_tile_y - s_view_pan_y) * 0.04f;
            float g_r = 0.20f, g_g = 0.95f, g_b = 0.35f;

            float m_fuel = 0.0f, m_prog = 0.0f;
            uint32_t m_in = 0, m_out = 0;
            bool has_machine_info = false;

            if (is_occupied) {
                has_machine_info = s_get_machine_info && s_get_machine_info(hover_tile_x, hover_tile_y, &m_fuel, &m_prog, &m_in, &m_out);
                if (has_machine_info && m_out > 0) {
                    g_r = 1.0f; g_g = 0.85f; g_b = 0.20f; // Dorado Factorio: listo para lootear
                } else {
                    g_r = 0.95f; g_g = 0.25f; g_b = 0.20f; // Rojo: ocupado
                }
            } else if (hovered_patch_type > 0) {
                if (hovered_patch_type == 1) { g_r = 0.35f; g_g = 0.65f; g_b = 0.95f; } // Hierro
                else if (hovered_patch_type == 2) { g_r = 0.95f; g_g = 0.55f; g_b = 0.25f; } // Cobre
                else if (hovered_patch_type == 3) { g_r = 0.35f; g_g = 0.35f; g_b = 0.35f; } // Carbón
                else if (hovered_patch_type == 4) { g_r = 0.85f; g_g = 0.82f; g_b = 0.75f; } // Piedra
            }

            float hw_tile = 0.02f;
            add_box_textured(dyn_verts, snap_mx, 0.002f, snap_mz - hw_tile, 0.04f, 0.001f, 0.002f, 1.0f, 1.0f, g_r, g_g, g_b, 0.8f);
            add_box_textured(dyn_verts, snap_mx, 0.002f, snap_mz + hw_tile, 0.04f, 0.001f, 0.002f, 1.0f, 1.0f, g_r, g_g, g_b, 0.8f);
            add_box_textured(dyn_verts, snap_mx - hw_tile, 0.002f, snap_mz, 0.002f, 0.001f, 0.04f, 1.0f, 1.0f, g_r, g_g, g_b, 0.8f);
            add_box_textured(dyn_verts, snap_mx + hw_tile, 0.002f, snap_mz, 0.002f, 0.001f, 0.04f, 1.0f, 1.0f, g_r, g_g, g_b, 0.8f);

            // Minado Manual con Pico (mantener Trigger presionado en veta de mineral)
            if (hovered_patch_type > 0 && !is_occupied && s_selected_build_tool == ENTITY_NONE) {
                if (trigger_held) {
                    if (fabsf(hover_tile_x - s_manual_mining_tile_x) > 0.5f || fabsf(hover_tile_y - s_manual_mining_tile_y) > 0.5f) {
                        s_manual_mining_timer = 0.0f;
                        s_manual_mining_tile_x = hover_tile_x;
                        s_manual_mining_tile_y = hover_tile_y;
                        s_last_mining_strike = 0.0f;
                    }
                    s_manual_mining_timer += dt;

                    // Golpes rítmicos de pico cada 0.24s
                    if (s_manual_mining_timer - s_last_mining_strike >= 0.24f || s_last_mining_strike > s_manual_mining_timer) {
                        s_last_mining_strike = s_manual_mining_timer;
                        trigger_controller_haptics(session, haptic_action, 0.55f, 25.0f);
                        play_spatial_sound(SFX_MINE, s_table_pos.x + snap_mx, s_table_pos.y, s_table_pos.z + snap_mz, 0.7f);
                        for (int k = 0; k < 6; ++k) {
                            spawn_particle(snap_mx, 0.015f, snap_mz,
                                           ((rand() % 100) - 50) * 0.001f, 0.035f + (rand() % 30) * 0.0005f, ((rand() % 100) - 50) * 0.001f,
                                           g_r, g_g, g_b, 0.9f, 0.008f, 0.5f);
                        }
                    }

                    // Anillo de progreso circular de minado alrededor del retículo
                    float fill_ratio = std::clamp(s_manual_mining_timer / 0.70f, 0.0f, 1.0f);
                    int arc_steps = (int)(fill_ratio * 20.0f);
                    float ring_r = 0.030f;
                    for (int s = 0; s < arc_steps; ++s) {
                        float a0 = (float)s * 6.2831853f / 20.0f;
                        float a1 = (float)(s + 1) * 6.2831853f / 20.0f;
                        dyn_verts.push_back({snap_mx + cosf(a0) * ring_r, 0.006f, snap_mz + sinf(a0) * ring_r, 0, 1, 0, 0, 0, 0.2f, 0.95f, 0.35f, 1.0f});
                        dyn_verts.push_back({snap_mx + cosf(a1) * ring_r, 0.006f, snap_mz + sinf(a1) * ring_r, 0, 1, 0, 0, 0, 0.2f, 0.95f, 0.35f, 1.0f});
                    }

                    if (s_manual_mining_timer >= 0.70f) {
                        s_manual_mining_timer = 0.0f;
                        s_last_mining_strike = 0.0f;
                        if (s_manual_mine) {
                            uint32_t mined = s_manual_mine(hover_tile_x, hover_tile_y);
                            LOGI("Minado manual completado en (%.1f, %.1f), item=%u", hover_tile_x, hover_tile_y, mined);
                            trigger_controller_haptics(session, haptic_action, 0.95f, 65.0f);
                            play_spatial_sound(SFX_CRAFT, s_table_pos.x + snap_mx, s_table_pos.y, s_table_pos.z + snap_mz, 1.0f);
                            for (int k = 0; k < 18; ++k) {
                                spawn_particle(snap_mx, 0.035f, snap_mz,
                                               ((rand() % 100) - 50) * 0.0012f, 0.06f + (rand() % 40) * 0.0008f, ((rand() % 100) - 50) * 0.0012f,
                                               1.0f, 0.90f, 0.25f, 0.95f, 0.010f, 0.7f);
                            }
                        }
                    }
                } else {
                    s_manual_mining_timer = 0.0f;
                    s_last_mining_strike = 0.0f;
                }
            } else {
                s_manual_mining_timer = 0.0f;
                s_last_mining_strike = 0.0f;
            }

            // GHOST PREVIEW holográfico del edificio seleccionado
            if (s_selected_build_tool != ENTITY_NONE && s_entity_meshes[s_selected_build_tool].vao != 0) {
                float ghost_trans[16], ghost_rot[16], ghost_local[16], ghost_world[16], ghost_mvp[16];
                mat4_rotate_y(ghost_rot, s_placement_yaw);
                mat4_translate(ghost_trans, snap_mx, 0.004f + sinf(anim_time * 4.0f) * 0.002f, snap_mz);
                mat4_multiply(ghost_trans, ghost_rot, ghost_local);
                mat4_multiply(active_model, ghost_local, ghost_world);
                mat4_multiply(vp_mat, ghost_world, ghost_mvp);

                glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, ghost_world);
                glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, ghost_mvp);
                glUniform1f(s_use_tex_loc, 0.0f);
                glUniform1f(s_lit_loc, 0.0f);
                glUniform3f(s_emissive_loc, g_r * 0.9f, g_g * 0.9f, g_b * 0.9f);

                glBindVertexArray(s_entity_meshes[s_selected_build_tool].vao);
                glDrawArrays(GL_TRIANGLES, 0, s_entity_meshes[s_selected_build_tool].vertex_count);
                glUniform3f(s_emissive_loc, 0.0f, 0.0f, 0.0f);
            }

            // ACCIONES DE CONSTRUCCIÓN, INTERACCIÓN Y MINADO (Fase 2)
            if (is_primary_eye) {
                // Al soltar el gatillo se cancela el arrastre en curso.
                if (!trigger_held) {
                    s_last_drag_tile_x = 9999;
                    s_last_drag_tile_y = 9999;
                }

                // 0. MANOS: coger del suelo, sacar de la caja, meter en el
                //    horno o en la caja.  El decide (Rust) y ejecuta contra
                //    el juego real.  Si no hay nada que hacer devuelve false y
                //    el resto del gatillo sigue siendo el raton de siempre.
                bool mano_hecha = false;
                if ((trigger_clicked || grip_clicked) && s_vr_hand_action) {
                    mano_hecha = s_vr_hand_action(hover_tile_x, hover_tile_y,
                                                   trigger_clicked ? 0u : 1u);
                    if (mano_hecha) {
                        LOGI("Mano VR: accion de inventario en (%.1f, %.1f) [%s]",
                             hover_tile_x, hover_tile_y, trigger_clicked ? "gatillo" : "gripo");
                        trigger_controller_haptics(session, haptic_action, 0.70f, 40.0f);
                        play_spatial_sound(SFX_CLICK, s_table_pos.x + snap_mx, s_table_pos.y, s_table_pos.z + snap_mz, 0.9f);
                        for (int k = 0; k < 14; ++k) {
                            spawn_particle(snap_mx, 0.04f, snap_mz,
                                           ((rand() % 100) - 50) * 0.0012f, 0.05f + (rand() % 40) * 0.0008f, ((rand() % 100) - 50) * 0.0012f,
                                           0.35f, 0.80f, 1.00f, 0.95f, 0.010f, 0.8f);
                        }
                    }
                }

                // 1. Colocar edificio con gatillo sobre casilla vacía
                if (!mano_hecha && trigger_clicked && s_selected_build_tool != ENTITY_NONE && !is_occupied) {
                    if (s_place_or_update_entity) {
                        s_place_or_update_entity(s_selected_build_tool, hover_tile_x, hover_tile_y, s_placement_yaw);
                        LOGI("Construido tipo=%d en tile (%.1f, %.1f) yaw=%.2f",
                             s_selected_build_tool, hover_tile_x, hover_tile_y, s_placement_yaw);
                        trigger_controller_haptics(session, haptic_action, 0.75f, 45.0f);
                        play_spatial_sound(SFX_PLACE, s_table_pos.x + snap_mx, s_table_pos.y, s_table_pos.z + snap_mz);
                        for (int k = 0; k < 20; ++k) {
                            spawn_particle(snap_mx, 0.05f, snap_mz,
                                           ((rand() % 100) - 50) * 0.001f, 0.04f + (rand() % 50) * 0.0005f, ((rand() % 100) - 50) * 0.001f,
                                           0.20f, 0.95f, 0.40f, 0.95f, 0.010f, 0.8f);
                        }
                    }
                    // El primer clic del arrastre marca el origen de la linea.
                    s_last_drag_tile_x = (int)hover_tile_x;
                    s_last_drag_tile_y = (int)hover_tile_y;
                }
                // 1.b Arrastre continuo: mantener el gatillo y mover el puntero
                //     tiende lineas de cintas transportadoras (o postes) de una
                //     sola vez, como se hace en Factorio con el raton.
                else if (!mano_hecha && trigger_held && s_selected_build_tool != ENTITY_NONE && !is_occupied &&
                         is_drag_build_tool(s_selected_build_tool)) {
                    int drag_tx = (int)hover_tile_x;
                    int drag_ty = (int)hover_tile_y;
                    if (drag_tx != s_last_drag_tile_x || drag_ty != s_last_drag_tile_y) {
                        s_last_drag_tile_x = drag_tx;
                        s_last_drag_tile_y = drag_ty;
                        if (s_place_or_update_entity) {
                            s_place_or_update_entity(s_selected_build_tool,
                                                     (float)drag_tx, (float)drag_ty, s_placement_yaw);
                            LOGI("Arrastre: tipo=%d en tile (%d, %d)", s_selected_build_tool, drag_tx, drag_ty);
                            // Feedback háptico suave: confirma la casilla sin distraer.
                            trigger_controller_haptics(session, haptic_action, 0.35f, 15.0f);
                        }
                    }
                }
                // 2. Interactuar / Lootear productos terminados o abastecer máquina ocupada
                else if (!mano_hecha && is_occupied && trigger_clicked && (s_selected_build_tool == ENTITY_NONE || (has_machine_info && m_out > 0))) {
                    if (s_interact_machine) {
                        uint32_t looted = s_interact_machine(hover_tile_x, hover_tile_y);
                        if (looted > 0) {
                            LOGI("Recogidas %u piezas de la máquina en (%.1f, %.1f)", looted, hover_tile_x, hover_tile_y);
                            trigger_controller_haptics(session, haptic_action, 0.90f, 55.0f);
                            play_spatial_sound(SFX_CRAFT, s_table_pos.x + snap_mx, s_table_pos.y, s_table_pos.z + snap_mz, 1.0f);
                            for (int k = 0; k < 20; ++k) {
                                spawn_particle(snap_mx, 0.07f, snap_mz,
                                               ((rand() % 100) - 50) * 0.0012f, 0.05f + (rand() % 50) * 0.0008f, ((rand() % 100) - 50) * 0.0012f,
                                               1.0f, 0.90f, 0.30f, 0.95f, 0.012f, 0.8f);
                            }
                        } else {
                            // Auto-abastecer combustible/mineral
                            trigger_controller_haptics(session, haptic_action, 0.40f, 30.0f);
                            play_spatial_sound(SFX_CLICK, s_table_pos.x + snap_mx, s_table_pos.y, s_table_pos.z + snap_mz, 0.8f);
                        }
                    }
                }
                // 3. Minar / Deconstruir máquina con botón Grip o modo demoler si no hay productos
                else if (!mano_hecha && is_occupied && (grip_clicked || (trigger_clicked && s_selected_build_tool == ENTITY_NONE && (!has_machine_info || m_out == 0)))) {
                    if (s_mine_entity) {
                        s_mine_entity(hover_tile_x, hover_tile_y);
                        LOGI("Minado/Deconstruido en tile (%.1f, %.1f)", hover_tile_x, hover_tile_y);
                        trigger_controller_haptics(session, haptic_action, 0.90f, 65.0f);
                        play_spatial_sound(SFX_MINE, s_table_pos.x + snap_mx, s_table_pos.y, s_table_pos.z + snap_mz);
                        for (int k = 0; k < 24; ++k) {
                            spawn_particle(snap_mx, 0.04f, snap_mz,
                                           ((rand() % 100) - 50) * 0.0015f, 0.05f + (rand() % 60) * 0.0008f, ((rand() % 100) - 50) * 0.0015f,
                                           0.95f, 0.45f, 0.15f, 0.95f, 0.012f, 0.9f);
                        }
                    }
                }
            }
        }

        // 4. Geometría del rayo láser
        float end_x = rx + dx * ray_len;
        float end_y = ry + dy * ray_len;
        float end_z = rz + dz * ray_len;
        float l_r = (has_ui_hit || has_table_hit) ? 0.15f : 0.85f;
        float l_g = (has_ui_hit || has_table_hit) ? 0.95f : 0.35f;
        float l_b = (has_ui_hit || has_table_hit) ? 1.00f : 0.20f;

        std::vector<DioramaVertex> laser_verts;
        add_box_textured(laser_verts, (rx + end_x) * 0.5f, (ry + end_y) * 0.5f, (rz + end_z) * 0.5f,
                         0.003f, 0.003f, ray_len, 1.0f, 1.0f, l_r, l_g, l_b, 0.92f);

        if (has_table_hit) {
            float ret_rad = 0.032f + sinf(anim_time * 6.5f) * 0.004f;
            for (int s = 0; s < 16; ++s) {
                float a0 = (float)s * 6.2831853f / 16.0f;
                float a1 = (float)(s + 1) * 6.2831853f / 16.0f;
                laser_verts.push_back({hit_stage_x + cosf(a0) * ret_rad, s_table_pos.y + 0.003f, hit_stage_z + sinf(a0) * ret_rad, 0, 1, 0, 0, 0, l_r, l_g, l_b, 0.95f});
                laser_verts.push_back({hit_stage_x + cosf(a1) * ret_rad, s_table_pos.y + 0.003f, hit_stage_z + sinf(a1) * ret_rad, 0, 1, 0, 0, 0, l_r, l_g, l_b, 0.95f});
            }
        }

        // 5. Tooltips Holográficos 3D en Mandos Pico (XrControllerHelp — Generals XR pattern)
        // Posicionado flotando 6cm encima del mando derecho mirando hacia el jugador
        float cx = rx;
        float cy = ry + 0.055f;
        float cz = rz;

        // Placa base holográfica flotante de HUD (#0e141a translúcido)
        add_box_textured(laser_verts, cx, cy, cz, 0.11f, 0.040f, 0.002f, 1.0f, 1.0f, 0.06f, 0.09f, 0.14f, 0.90f);
        // Borde superior cian Factorio
        add_box_textured(laser_verts, cx, cy + 0.018f, cz + 0.002f, 0.106f, 0.003f, 0.001f, 1.0f, 1.0f, 0.20f, 0.85f, 1.00f, 0.95f);

        // Badge 1 (Superior Izq): Trigger -> Acción / Construir (Cian)
        add_box_textured(laser_verts, cx - 0.026f, cy + 0.007f, cz + 0.002f, 0.044f, 0.012f, 0.001f, 1.0f, 1.0f, 0.15f, 0.75f, 0.95f, 0.95f);
        // Badge 2 (Superior Der): Grip -> Mover Mesa / Demoler (Naranja)
        add_box_textured(laser_verts, cx + 0.026f, cy + 0.007f, cz + 0.002f, 0.044f, 0.012f, 0.001f, 1.0f, 1.0f, 0.95f, 0.55f, 0.15f, 0.95f);
        // Badge 3 (Inferior Izq): Stick -> Rotar Edificio / Altura (Amarillo)
        add_box_textured(laser_verts, cx - 0.026f, cy - 0.008f, cz + 0.002f, 0.044f, 0.012f, 0.001f, 1.0f, 1.0f, 0.95f, 0.85f, 0.20f, 0.95f);
        // Badge 4 (Inferior Der): 2 Grips -> Zoom & Rotar Mesa (Púrpura)
        add_box_textured(laser_verts, cx + 0.026f, cy - 0.008f, cz + 0.002f, 0.044f, 0.012f, 0.001f, 1.0f, 1.0f, 0.75f, 0.35f, 0.95f, 0.95f);

        // Renderizar láser con MVP de mundo
        float ident_mat[16];
        matIdentity(ident_mat);
        glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, ident_mat);
        glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, vp_mat);
        glUniform1f(s_enable_clipping_loc, 0.0f);
        glUniform1f(s_lit_loc, 0.0f);
        glUniform3f(s_emissive_loc, l_r, l_g, l_b);

        if (!laser_verts.empty()) {
            glBindVertexArray(s_dynamic_vao);
            glBindBuffer(GL_ARRAY_BUFFER, s_dynamic_vbo);
            glBufferSubData(GL_ARRAY_BUFFER, 0, laser_verts.size() * sizeof(DioramaVertex), laser_verts.data());
            glDrawArrays(GL_TRIANGLES, 0, (GLsizei)laser_verts.size());
        }
    }

    // Renderizar elementos dinámicos de la mesa (cables, piezas móviles, partículas)
    if (!dyn_verts.empty()) {
        float dyn_mvp[16];
        mat4_multiply(vp_mat, active_model, dyn_mvp);
        glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, active_model);
        glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, dyn_mvp);
        glUniform1f(s_enable_clipping_loc, s_observer_mode ? 0.0f : 1.0f);
        glUniform1f(s_use_tex_loc, 0.0f);
        glUniform1f(s_lit_loc, 1.0f);
        glUniform3f(s_emissive_loc, 0.0f, 0.0f, 0.0f);

        glBindVertexArray(s_dynamic_vao);
        glBindBuffer(GL_ARRAY_BUFFER, s_dynamic_vbo);
        glBufferSubData(GL_ARRAY_BUFFER, 0, dyn_verts.size() * sizeof(DioramaVertex), dyn_verts.data());
        glDrawArrays(GL_TRIANGLES, 0, (GLsizei)dyn_verts.size());
    }

    // El render real del juego (si el juego lo esta mandando) se pinta como un
    // panel al fondo de la mesa: es la imagen que compone el motor, con el
    // cubo 3D dentro. Va lo ultimo para que quede por encima de la mesa.
    draw_game_panel(vp_mat, active_model, is_primary_eye);

    glBindVertexArray(0);
    glBindBuffer(GL_ARRAY_BUFFER, 0);
}

// ============================================================================
// RENDER REAL DEL JUEGO (protocolo XFV1)
// ============================================================================
//
// El inyector (dentro del proceso del juego) manda por TCP el framebuffer del
// mundo: lo que el jugador ve de verdad, con el cubo 3D del modulo 3D dentro.
// Aqui se recibe y se pinta como un panel en MR.
//
// Protocolo XFV1 (little-endian):
//   0..4    magic u32 = 0x31564658 ("XFV1")
//   4..8    numero de frame u32
//   8..10   ancho u16
//   10..12  alto u16
//   12      flags u8
//   13      formato u8 (0 = RGBA8888)
//   14..16  reservado u16
//   carga: ancho*alto*4 bytes RGBA (filas de abajo arriba, como en GL)
//
// El panel se dibuja con un pequeño desplazamiento de UV por ojo: eso es el
// efecto "TV 3D", y da volumen al cubo que pinta el inyector.

static const uint16_t JUEGO_PUERTO = 27020;
/// Desplazamiento horizontal de la UV por ojo (fraccion del ancho de la imagen).
static const float JUEGO_PARALAJE = 0.018f;
/// Altura del panel sobre la mesa, en metros.
static const float JUEGO_ALTURA = 0.30f;
/// Distancia del panel al borde lejano de la mesa, en metros.
static const float JUEGO_PROFUNDIDAD = -0.56f;
/// Ancho del panel, en metros (16:9 con la altura que sale de la textura).
static const float JUEGO_ANCHO = 1.00f;

/// Frame recibido del juego.
struct GameFrame {
    std::vector<uint8_t> pix;
    uint16_t w = 0;
    uint16_t h = 0;
    uint32_t numero = 0;
    bool nuevo = false;
};

static std::mutex s_game_mutex;
static GameFrame s_game_frame;
static std::atomic<bool> s_game_corriendo{false};
static std::atomic<uint64_t> s_game_frames{0};
static GLuint s_tex_juego = 0;
static uint16_t s_tex_juego_w = 0;
static uint16_t s_tex_juego_h = 0;
static GLuint s_vao_juego = 0;
static GLuint s_vbo_juego = 0;
static int s_juego_vertices = 0;
static pthread_t s_game_thread = 0;

/// Lee exactamente N bytes del socket. Devuelve false si se corta.
static bool leer_exacto(int fd, uint8_t* buf, size_t n) {
    size_t leidos = 0;
    while (leidos < n) {
        ssize_t r = ::read(fd, buf + leidos, n - leidos);
        if (r <= 0) {
            if (r < 0 && errno == EINTR) continue;
            return false;
        }
        leidos += (size_t)r;
    }
    return true;
}

/// Hilo receptor del render del juego.
///
/// Escucha en 127.0.0.1:27020 y guarda el ultimo frame recibido. NO toca el
/// GL: el hilo de OpenXR es el unico que puede subir la textura.
static void* game_receiver_thread(void* /*arg*/) {
    while (s_game_corriendo.load()) {
        int srv = socket(AF_INET, SOCK_STREAM, 0);
        if (srv < 0) {
            LOGE("No se pudo crear el socket del render del juego");
            sleep(1);
            continue;
        }
        int uno = 1;
        setsockopt(srv, SOL_SOCKET, SO_REUSEADDR, &uno, sizeof(uno));

        sockaddr_in addr{};
        addr.sin_family = AF_INET;
        addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        addr.sin_port = htons(JUEGO_PUERTO);
        if (bind(srv, (sockaddr*)&addr, sizeof(addr)) < 0 || listen(srv, 1) < 0) {
            LOGE("No se pudo escuchar el render del juego en el puerto %d", JUEGO_PUERTO);
            ::close(srv);
            sleep(2);
            continue;
        }
        LOGI("Escuchando el render real del juego en 127.0.0.1:%d", JUEGO_PUERTO);

        while (s_game_corriendo.load()) {
            int cli = accept(srv, nullptr, nullptr);
            if (cli < 0) {
                if (errno == EINTR) continue;
                break;
            }
            LOGI("El juego se ha conectado: empieza el render real");
            std::vector<uint8_t> cab(16);
            std::vector<uint8_t> pix;
            while (s_game_corriendo.load()) {
                if (!leer_exacto(cli, cab.data(), cab.size())) break;
                uint32_t magic = 0, numero = 0;
                uint16_t w = 0, h = 0;
                memcpy(&magic, cab.data() + 0, 4);
                memcpy(&numero, cab.data() + 4, 4);
                memcpy(&w, cab.data() + 8, 2);
                memcpy(&h, cab.data() + 10, 2);
                const uint8_t formato = cab[13];
                if (magic != 0x31564658u) {
                    LOGE("Cabecera desconocida del render del juego: 0x%08x", magic);
                    break;
                }
                if (w == 0 || h == 0 || formato != 0) break;
                const size_t tam = (size_t)w * (size_t)h * 4;
                // El buffer se reutiliza: al intercambiarlo con el frame
                // guardado, el siguiente frame vuelve con la capacidad ya
                // reservada (900 KB por frame es mucha basura).
                pix.resize(tam);
                if (!leer_exacto(cli, pix.data(), tam)) break;
                {
                    std::lock_guard<std::mutex> lock(s_game_mutex);
                    s_game_frame.pix.swap(pix);
                    s_game_frame.w = w;
                    s_game_frame.h = h;
                    s_game_frame.numero = numero;
                    s_game_frame.nuevo = true;
                }
                s_game_frames.fetch_add(1);
            }
            ::close(cli);
            LOGI("Se ha cortado la conexion del render del juego");
        }
        ::close(srv);
    }
    return nullptr;
}

/// Crea el quad del panel (se hace una sola vez).
static void construir_panel_juego() {
    if (s_vao_juego != 0) return;
    std::vector<DioramaVertex> verts;
    const float w = JUEGO_ANCHO * 0.5f;
    const float h = w * 9.0f / 16.0f;
    auto push = [&](float x, float y, float u, float v) {
        DioramaVertex d{};
        d.x = x; d.y = y; d.z = 0.0f;
        d.nx = 0.0f; d.ny = 0.0f; d.nz = 1.0f;
        d.u = u; d.v = v;
        d.r = 1.0f; d.g = 1.0f; d.b = 1.0f; d.a = 1.0f;
        verts.push_back(d);
    };
    // Cara que mira a +Z.
    //
    // La V va INVERTIDA a proposito: el motor dibuja el mundo con la Y
    // invertida, asi que en el framebuffer la fila 0 es la de ARRIBA de la
    // imagen (comprobado comparando el frame que viaja por el socket con una
    // captura de la pantalla real del juego).  Al subir el buffer tal cual a
    // la textura, la t=0 queda en la fila 0, asi que la t=0 tiene que pintar
    // la parte de ARRIBA del panel.
    push(-w, -h, 0.0f, 1.0f);
    push( w, -h, 1.0f, 1.0f);
    push( w,  h, 1.0f, 0.0f);
    push(-w, -h, 0.0f, 1.0f);
    push( w,  h, 1.0f, 0.0f);
    push(-w,  h, 0.0f, 0.0f);
    s_juego_vertices = (int)verts.size();
    s_vao_juego = create_mesh_vao(verts, s_vbo_juego);
}

/// Sube a la textura el frame mas reciente (si hay uno nuevo).
static void subir_frame_juego() {
    // El buffer del hilo de render y el del hilo receptor se intercambian:
    // asi los dos buffers (900 KB cada uno) van y vienen en vez de estar
    // reservandose y liberandose en cada frame.
    static std::vector<uint8_t> s_pix;
    uint16_t w = 0, h = 0;
    {
        std::lock_guard<std::mutex> lock(s_game_mutex);
        if (!s_game_frame.nuevo) return;
        s_pix.swap(s_game_frame.pix);
        w = s_game_frame.w;
        h = s_game_frame.h;
        s_game_frame.nuevo = false;
    }
    if (w == 0 || h == 0 || s_pix.empty()) return;

    if (s_tex_juego == 0) {
        glGenTextures(1, &s_tex_juego);
        glBindTexture(GL_TEXTURE_2D, s_tex_juego);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    }
    glBindTexture(GL_TEXTURE_2D, s_tex_juego);
    if (w != s_tex_juego_w || h != s_tex_juego_h) {
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, s_pix.data());
        s_tex_juego_w = w;
        s_tex_juego_h = h;
    } else {
        glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, s_pix.data());
    }
    glBindTexture(GL_TEXTURE_2D, 0);
}

/// Pinta el panel con el render real del juego.
///
/// El desplazamiento por ojo (`u_uv_offset`) es lo que da el paralaje: la
/// imagen se ve como si fuera una ventana al mundo y el cubo 3D del inyector
/// gana volumen.
static void draw_game_panel(const float* vp_mat, const float* model_mesa, bool ojo_primario) {
    // En modo observador el mundo esta a escala 1:1 (x25) y el panel de la
    // mesa no tiene sentido ahi.
    if (s_observer_mode) return;
    subir_frame_juego();
    if (s_tex_juego == 0) return;
    construir_panel_juego();
    if (s_vao_juego == 0) return;

    // Posicion: en el espacio de la mesa, en el borde lejano y por encima.
    float m_pos[16], m_panel[16];
    mat4_translate(m_pos, 0.0f, JUEGO_ALTURA, JUEGO_PROFUNDIDAD);
    mat4_multiply(model_mesa, m_pos, m_panel);

    float mvp[16];
    mat4_multiply(vp_mat, m_panel, mvp);

    glUseProgram(s_program);
    glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, m_panel);
    glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, mvp);
    glUniform1f(s_use_tex_loc, 1.0f);
    glUniform1f(s_lit_loc, 0.0f);
    glUniform3f(s_emissive_loc, 1.0f, 1.0f, 1.0f);
    glUniform1f(s_enable_clipping_loc, 0.0f);
    // El ojo primario ve la imagen sin desplazar; el otro la desplaza un poco.
    const float desfase = ojo_primario ? -JUEGO_PARALAJE : JUEGO_PARALAJE;
    glUniform2f(s_uv_offset_loc, desfase, 0.0f);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, s_tex_juego);
    glUniform1i(s_tex_loc, 0);

    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    glDepthMask(GL_FALSE);
    glBindVertexArray(s_vao_juego);
    glDrawArrays(GL_TRIANGLES, 0, s_juego_vertices);
    glDepthMask(GL_TRUE);
    glDisable(GL_BLEND);

    glBindVertexArray(0);
    glBindBuffer(GL_ARRAY_BUFFER, 0);
}

// ============================================================================
// HILO PRINCIPAL DE OPENXR
// ============================================================================

static void* openxr_thread_main(void* arg) {
    ANativeActivity* activity = (ANativeActivity*)arg;
    LOGI("=== Iniciando hilo OpenXR Standalone para Pico 4 Ultra ===");

    JNIEnv* jni_env = nullptr;
    JavaVMAttachArgs attachArgs;
    attachArgs.version = JNI_VERSION_1_6;
    attachArgs.name = "XFactorVR_OpenXR_Thread";
    attachArgs.group = nullptr;
    jint attachRes = activity->vm->AttachCurrentThread(&jni_env, &attachArgs);
    LOGI("AttachCurrentThread resultado: %d, env: %p", attachRes, jni_env);

    // Validar matemáticas 3D y transformaciones de OpenXR estilo Generals XR
    selfCheckMath();

    // Receptor del render real del juego: el inyector (dentro del proceso
    // del juego) se conecta a este puerto y manda los frames.
    s_game_corriendo.store(true);
    if (pthread_create(&s_game_thread, nullptr, game_receiver_thread, nullptr) != 0) {
        LOGE("No se pudo arrancar el hilo receptor del render del juego");
        s_game_corriendo.store(false);
    }

    // 1. Inicializar Loader Android
    PFN_xrInitializeLoaderKHR xrInitializeLoaderKHR = nullptr;
    xrGetInstanceProcAddr(XR_NULL_HANDLE, "xrInitializeLoaderKHR", (PFN_xrVoidFunction*)&xrInitializeLoaderKHR);
    if (xrInitializeLoaderKHR) {
        XrLoaderInitInfoAndroidKHR loaderInit = {
            .type = XR_TYPE_LOADER_INIT_INFO_ANDROID_KHR,
            .next = nullptr,
            .applicationVM = activity->vm,
            .applicationContext = activity->clazz,
        };
        XrResult res = xrInitializeLoaderKHR((const XrLoaderInitInfoBaseHeaderKHR*)&loaderInit);
        LOGI("xrInitializeLoaderKHR resultado: %d", res);
    }

    // 2. Crear Instancia OpenXR
    // 2.1 Negocio de extensiones: el runtime PICO 3.0.0 no soporta necesariamente
    //     todas las extensiones deseadas. Habilitar una extensión ausente hace
    //     fallar xrCreateInstance, así que se consultan primero y se filtran.
    uint32_t extCount = 0;
    xrEnumerateInstanceExtensionProperties(nullptr, 0, &extCount, nullptr);
    std::vector<XrExtensionProperties> available(extCount, { .type = XR_TYPE_EXTENSION_PROPERTIES });
    if (extCount > 0) {
        xrEnumerateInstanceExtensionProperties(nullptr, extCount, &extCount, available.data());
    }

    auto is_supported = [&](const char* name) {
        for (const auto& ext : available) {
            if (strcmp(ext.extensionName, name) == 0) return true;
        }
        return false;
    };

    // Extensiones requeridas (si el runtime no ofrece las de OpenGL ES, no hay motor).
    const char* const requiredExtensions[] = {
        "XR_KHR_android_create_instance",
        "XR_KHR_opengl_es_enable",
    };
    // Extensiones opcionales de PICO (passthrough, mando, rendimiento).
    const char* const optionalExtensions[] = {
        "XR_FB_passthrough",
        "XR_BD_controller_interaction",
        "XR_EXT_performance_settings",
        "XR_FB_display_refresh_rate",
    };

    std::vector<const char*> enabledExtensions;
    for (const char* name : requiredExtensions) {
        if (!is_supported(name)) {
            LOGE("Extensión OpenXR obligatoria ausente: %s", name);
            return nullptr;
        }
        enabledExtensions.push_back(name);
    }

    s_has_fb_passthrough = is_supported("XR_FB_passthrough");
    s_has_bd_controller  = is_supported("XR_BD_controller_interaction");
    s_has_ext_perf       = is_supported("XR_EXT_performance_settings");
    s_has_fb_refresh     = is_supported("XR_FB_display_refresh_rate");

    for (const char* name : optionalExtensions) {
        if (is_supported(name)) {
            enabledExtensions.push_back(name);
            LOGI("Extensión OpenXR habilitada: %s", name);
        } else {
            LOGI("Extensión OpenXR no soportada por el runtime (omitida): %s", name);
        }
    }

    XrInstanceCreateInfoAndroidKHR instanceAndroid = {
        .type = XR_TYPE_INSTANCE_CREATE_INFO_ANDROID_KHR,
        .next = nullptr,
        .applicationVM = activity->vm,
        .applicationActivity = activity->clazz,
    };

    XrInstanceCreateInfo instanceInfo = {
        .type = XR_TYPE_INSTANCE_CREATE_INFO,
        .next = &instanceAndroid,
        .createFlags = 0,
        .applicationInfo = {
            .applicationName = "XFactorVR Tabletop",
            .applicationVersion = 1,
            .engineName = "XFactorVR",
            .engineVersion = 1,
            .apiVersion = XR_CURRENT_API_VERSION,
        },
        .enabledExtensionCount = (uint32_t)enabledExtensions.size(),
        .enabledExtensionNames = enabledExtensions.data(),
    };

    XrInstance instance = XR_NULL_HANDLE;
    XrResult res = xrCreateInstance(&instanceInfo, &instance);
    if (res != XR_SUCCESS || instance == XR_NULL_HANDLE) {
        LOGE("Error al crear XrInstance: %d", res);
        return nullptr;
    }
    LOGI("XrInstance creada exitosamente");

    // 3. Obtener System ID para HMD
    XrSystemGetInfo systemInfo = {
        .type = XR_TYPE_SYSTEM_GET_INFO,
        .next = nullptr,
        .formFactor = XR_FORM_FACTOR_HEAD_MOUNTED_DISPLAY,
    };
    XrSystemId systemId = XR_NULL_SYSTEM_ID;
    res = xrGetSystem(instance, &systemInfo, &systemId);
    if (res != XR_SUCCESS) {
        LOGE("Error en xrGetSystem: %d", res);
        return nullptr;
    }
    LOGI("XrSystemId obtenido: %llu", (unsigned long long)systemId);

    // 4. Requerimientos OpenGL ES
    PFN_xrGetOpenGLESGraphicsRequirementsKHR xrGetOpenGLESGraphicsRequirementsKHR = nullptr;
    xrGetInstanceProcAddr(instance, "xrGetOpenGLESGraphicsRequirementsKHR", (PFN_xrVoidFunction*)&xrGetOpenGLESGraphicsRequirementsKHR);
    if (xrGetOpenGLESGraphicsRequirementsKHR) {
        XrGraphicsRequirementsOpenGLESKHR req = { .type = XR_TYPE_GRAPHICS_REQUIREMENTS_OPENGL_ES_KHR };
        xrGetOpenGLESGraphicsRequirementsKHR(instance, systemId, &req);
    }

    // 5. Inicializar EGL y contexto OpenGL ES 3.2
    EGLDisplay egl_display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    eglInitialize(egl_display, nullptr, nullptr);

    const EGLint configAttribs[] = {
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, 8,
        EGL_DEPTH_SIZE, 24,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
        EGL_NONE
    };
    EGLConfig egl_config;
    EGLint numConfigs;
    eglChooseConfig(egl_display, configAttribs, &egl_config, 1, &numConfigs);

    const EGLint contextAttribs[] = {
        EGL_CONTEXT_CLIENT_VERSION, 3,
        EGL_NONE
    };
    EGLContext egl_context = eglCreateContext(egl_display, egl_config, EGL_NO_CONTEXT, contextAttribs);

    EGLint pbufferAttribs[] = { EGL_WIDTH, 16, EGL_HEIGHT, 16, EGL_NONE };
    EGLSurface egl_surface = eglCreatePbufferSurface(egl_display, egl_config, pbufferAttribs);
    eglMakeCurrent(egl_display, egl_surface, egl_surface, egl_context);
    LOGI("Contexto EGL / OpenGL ES 3.2 creado y activo");

    // 6. Crear XrSession
    XrGraphicsBindingOpenGLESAndroidKHR glesBinding = {
        .type = XR_TYPE_GRAPHICS_BINDING_OPENGL_ES_ANDROID_KHR,
        .next = nullptr,
        .display = egl_display,
        .config = egl_config,
        .context = egl_context,
    };

    XrSessionCreateInfo sessionInfo = {
        .type = XR_TYPE_SESSION_CREATE_INFO,
        .next = &glesBinding,
        .createFlags = 0,
        .systemId = systemId,
    };

    XrSession session = XR_NULL_HANDLE;
    res = xrCreateSession(instance, &sessionInfo, &session);
    if (res != XR_SUCCESS || session == XR_NULL_HANDLE) {
        LOGE("Error al crear XrSession: %d", res);
        return nullptr;
    }
    LOGI("XrSession creada exitosamente");

    // 6.1 Capa de Passthrough (XR_FB_passthrough)
    // La capa de passthrough debe ir PRIMERA en el arreglo de capas de xrEndFrame:
    // el runtime la usa como fondo y la capa de proyección se compone encima.
    XrPassthroughLayerFB passthroughLayer = XR_NULL_HANDLE;
    if (s_has_fb_passthrough) {
        PFN_xrCreatePassthroughLayerFB xrCreatePassthroughLayerFB = nullptr;
        xrGetInstanceProcAddr(instance, "xrCreatePassthroughLayerFB",
                              (PFN_xrVoidFunction*)&xrCreatePassthroughLayerFB);
        if (xrCreatePassthroughLayerFB) {
            XrPassthroughLayerCreateInfoFB ptInfo = {
                .type = XR_TYPE_PASSTHROUGH_LAYER_CREATE_INFO_FB,
                .next = nullptr,
                // IS_RUNNING_AT_CREATION: la capa arranca activa para no parpadear
                // durante los primeros frames mientras compone la mesa.
                .flags = XR_PASSTHROUGH_IS_RUNNING_AT_CREATION_BIT_FB,
                .purpose = XR_PASSTHROUGH_LAYER_PURPOSE_RECONSTRUCTION_FB,
            };
            res = xrCreatePassthroughLayerFB(session, &ptInfo, &passthroughLayer);
            if (XR_SUCCEEDED(res)) {
                LOGI("Capa de Passthrough (XR_FB_passthrough) creada");
            } else {
                passthroughLayer = XR_NULL_HANDLE;
                LOGI("No se pudo crear la capa de Passthrough (res=%d); se usará ALPHA_BLEND", res);
            }
        }
    }

    // 7. Crear espacio de referencia STAGE (Fijo al suelo de la habitación)
    XrReferenceSpaceCreateInfo spaceInfo = {
        .type = XR_TYPE_REFERENCE_SPACE_CREATE_INFO,
        .next = nullptr,
        .referenceSpaceType = XR_REFERENCE_SPACE_TYPE_STAGE,
        .poseInReferenceSpace = {
            .orientation = { 0.0f, 0.0f, 0.0f, 1.0f },
            .position = { 0.0f, 0.0f, 0.0f }
        }
    };
    XrSpace appSpace = XR_NULL_HANDLE;
    res = xrCreateReferenceSpace(session, &spaceInfo, &appSpace);
    if (res != XR_SUCCESS) {
        LOGI("Fallo crear espacio STAGE, recurriendo a LOCAL: %d", res);
        spaceInfo.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_LOCAL;
        xrCreateReferenceSpace(session, &spaceInfo, &appSpace);
    } else {
        LOGI("Espacio de referencia STAGE (world-locked al suelo) creado correctamente");
    }

    // 7.1 ActionSet completo para mandos 6DoF (Aim, Trigger, Grip, Thumbstick)
    XrActionSetCreateInfo actionSetInfo = {
        .type = XR_TYPE_ACTION_SET_CREATE_INFO,
        .next = nullptr,
        .priority = 0,
    };
    strncpy(actionSetInfo.actionSetName, "gameplay", XR_MAX_ACTION_SET_NAME_SIZE);
    strncpy(actionSetInfo.localizedActionSetName, "Gameplay", XR_MAX_LOCALIZED_ACTION_SET_NAME_SIZE);
    XrActionSet actionSet = XR_NULL_HANDLE;
    xrCreateActionSet(instance, &actionSetInfo, &actionSet);

    // Aim Pose
    XrActionCreateInfo aimInfo = { .type = XR_TYPE_ACTION_CREATE_INFO, .actionType = XR_ACTION_TYPE_POSE_INPUT };
    strncpy(aimInfo.actionName, "right_aim", XR_MAX_ACTION_NAME_SIZE);
    strncpy(aimInfo.localizedActionName, "Right Aim", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction rightAimAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &aimInfo, &rightAimAction);

    // Left Aim Pose (para interacción bimanual)
    strncpy(aimInfo.actionName, "left_aim", XR_MAX_ACTION_NAME_SIZE);
    strncpy(aimInfo.localizedActionName, "Left Aim", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction leftAimAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &aimInfo, &leftAimAction);

    // Trigger (Right)
    XrActionCreateInfo trigInfo = { .type = XR_TYPE_ACTION_CREATE_INFO, .actionType = XR_ACTION_TYPE_FLOAT_INPUT };
    strncpy(trigInfo.actionName, "right_trigger", XR_MAX_ACTION_NAME_SIZE);
    strncpy(trigInfo.localizedActionName, "Right Trigger", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction rightTrigAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &trigInfo, &rightTrigAction);

    // Grip (Right & Left - para agarrar la mesa y zoom bimanual)
    XrActionCreateInfo gripInfo = { .type = XR_TYPE_ACTION_CREATE_INFO, .actionType = XR_ACTION_TYPE_FLOAT_INPUT };
    strncpy(gripInfo.actionName, "right_grip", XR_MAX_ACTION_NAME_SIZE);
    strncpy(gripInfo.localizedActionName, "Right Grip", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction rightGripAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &gripInfo, &rightGripAction);

    strncpy(gripInfo.actionName, "left_grip", XR_MAX_ACTION_NAME_SIZE);
    strncpy(gripInfo.localizedActionName, "Left Grip", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction leftGripAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &gripInfo, &leftGripAction);

    // Thumbsticks
    XrActionCreateInfo stickInfo = { .type = XR_TYPE_ACTION_CREATE_INFO, .actionType = XR_ACTION_TYPE_VECTOR2F_INPUT };
    strncpy(stickInfo.actionName, "right_stick", XR_MAX_ACTION_NAME_SIZE);
    strncpy(stickInfo.localizedActionName, "Right Stick", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction rightStickAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &stickInfo, &rightStickAction);

    // Stick izquierdo: desplazamiento del mapa (pan de vista). El stick derecho
    // ya esta ocupado por la altura de la mesa y el giro de la herramienta de
    // colocacion, asi que el pan necesita su propio stick para no pelearse.
    strncpy(stickInfo.actionName, "left_stick", XR_MAX_ACTION_NAME_SIZE);
    strncpy(stickInfo.localizedActionName, "Left Stick", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction leftStickAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &stickInfo, &leftStickAction);

    // Haptics (Vibración háptica táctil)
    XrActionCreateInfo hapticInfo = { .type = XR_TYPE_ACTION_CREATE_INFO, .actionType = XR_ACTION_TYPE_VIBRATION_OUTPUT };
    strncpy(hapticInfo.actionName, "right_haptic", XR_MAX_ACTION_NAME_SIZE);
    strncpy(hapticInfo.localizedActionName, "Right Haptic", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction rightHapticAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &hapticInfo, &rightHapticAction);

    // Bindings de interacción
    XrPath simpleProfile = XR_NULL_PATH, oculusProfile = XR_NULL_PATH, picoProfile = XR_NULL_PATH;
    XrPath pico4sProfile = XR_NULL_PATH;
    xrStringToPath(instance, "/interaction_profiles/khr/simple_controller", &simpleProfile);
    xrStringToPath(instance, "/interaction_profiles/oculus/touch_controller", &oculusProfile);
    xrStringToPath(instance, "/interaction_profiles/bytedance/pico4_controller", &picoProfile);
    // Pico 4 Ultra (controlador "Hawk"): perfil propio, distinto del Pico 4.
    xrStringToPath(instance, "/interaction_profiles/bytedance/pico4s_controller", &pico4sProfile);

    XrPath rightAimPath = XR_NULL_PATH, rightTrigPath = XR_NULL_PATH, rightGripPath = XR_NULL_PATH, rightStickPath = XR_NULL_PATH;
    xrStringToPath(instance, "/user/hand/right/input/aim/pose", &rightAimPath);
    xrStringToPath(instance, "/user/hand/right/input/trigger/value", &rightTrigPath);
    xrStringToPath(instance, "/user/hand/right/input/squeeze/value", &rightGripPath);
    xrStringToPath(instance, "/user/hand/right/input/thumbstick", &rightStickPath);

    XrPath leftAimPath = XR_NULL_PATH, leftGripPath = XR_NULL_PATH, leftStickPath = XR_NULL_PATH, rightHapticPath = XR_NULL_PATH;
    xrStringToPath(instance, "/user/hand/left/input/aim/pose", &leftAimPath);
    xrStringToPath(instance, "/user/hand/left/input/squeeze/value", &leftGripPath);
    xrStringToPath(instance, "/user/hand/left/input/thumbstick", &leftStickPath);
    xrStringToPath(instance, "/user/hand/right/output/haptic", &rightHapticPath);

    XrPath simpleClickPath = XR_NULL_PATH;
    xrStringToPath(instance, "/user/hand/right/input/select/click", &simpleClickPath);

    XrActionSuggestedBinding simpleBindings[] = {
        { rightAimAction, rightAimPath },
        { rightTrigAction, simpleClickPath }
    };
    XrInteractionProfileSuggestedBinding simpleSuggested = {
        .type = XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING,
        .interactionProfile = simpleProfile,
        .countSuggestedBindings = 2,
        .suggestedBindings = simpleBindings,
    };
    xrSuggestInteractionProfileBindings(instance, &simpleSuggested);

    XrActionSuggestedBinding touchBindings[] = {
        { rightAimAction, rightAimPath },
        { rightTrigAction, rightTrigPath },
        { rightGripAction, rightGripPath },
        { rightStickAction, rightStickPath },
        { leftAimAction, leftAimPath },
        { leftGripAction, leftGripPath },
        { leftStickAction, leftStickPath },
        { rightHapticAction, rightHapticPath }
    };
    XrInteractionProfileSuggestedBinding touchSuggested = {
        .type = XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING,
        .interactionProfile = oculusProfile,
        .countSuggestedBindings = 8,
        .suggestedBindings = touchBindings,
    };
    xrSuggestInteractionProfileBindings(instance, &touchSuggested);

    XrInteractionProfileSuggestedBinding picoSuggested = {
        .type = XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING,
        .interactionProfile = picoProfile,
        .countSuggestedBindings = 8,
        .suggestedBindings = touchBindings,
    };
    xrSuggestInteractionProfileBindings(instance, &picoSuggested);

    // Perfil del mando de la Pico 4 Ultra (Hawk). Sin este binding el visor no
    // entrega poses de puntero y el gameplay no responde al controlador.
    if (pico4sProfile != XR_NULL_PATH) {
        XrInteractionProfileSuggestedBinding pico4sSuggested = {
            .type = XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING,
            .interactionProfile = pico4sProfile,
            .countSuggestedBindings = 8,
            .suggestedBindings = touchBindings,
        };
        XrResult bindRes = xrSuggestInteractionProfileBindings(instance, &pico4sSuggested);
        if (XR_SUCCEEDED(bindRes)) {
            LOGI("Bindings del perfil Pico 4 Ultra (pico4s_controller) registrados");
        } else {
            LOGI("Perfil pico4s_controller no aceptado por el runtime (res=%d)", bindRes);
        }
    }

    XrSessionActionSetsAttachInfo attachInfo = {
        .type = XR_TYPE_SESSION_ACTION_SETS_ATTACH_INFO,
        .countActionSets = 1,
        .actionSets = &actionSet,
    };
    xrAttachSessionActionSets(session, &attachInfo);

    XrActionSpaceCreateInfo actionSpaceInfo = {
        .type = XR_TYPE_ACTION_SPACE_CREATE_INFO,
        .action = rightAimAction,
        .subactionPath = XR_NULL_PATH,
        .poseInActionSpace = {
            .orientation = { 0.0f, 0.0f, 0.0f, 1.0f },
            .position = { 0.0f, 0.0f, 0.0f },
        },
    };
    XrSpace rightAimSpace = XR_NULL_HANDLE;
    xrCreateActionSpace(session, &actionSpaceInfo, &rightAimSpace);

    actionSpaceInfo.action = leftAimAction;
    XrSpace leftAimSpace = XR_NULL_HANDLE;
    xrCreateActionSpace(session, &actionSpaceInfo, &leftAimSpace);

    // 8. Swapchains con Depth Buffer
    uint32_t viewCount = 0;
    xrEnumerateViewConfigurationViews(instance, systemId, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, 0, &viewCount, nullptr);
    std::vector<XrViewConfigurationView> configViews(viewCount, { .type = XR_TYPE_VIEW_CONFIGURATION_VIEW });
    xrEnumerateViewConfigurationViews(instance, systemId, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, viewCount, &viewCount, configViews.data());

    std::vector<EyeSwapchain> swapchains(viewCount);
    for (uint32_t i = 0; i < viewCount; ++i) {
        swapchains[i].width = configViews[i].recommendedImageRectWidth;
        swapchains[i].height = configViews[i].recommendedImageRectHeight;

        XrSwapchainCreateInfo swapchainInfo = {
            .type = XR_TYPE_SWAPCHAIN_CREATE_INFO,
            .usageFlags = XR_SWAPCHAIN_USAGE_COLOR_ATTACHMENT_BIT | XR_SWAPCHAIN_USAGE_SAMPLED_BIT,
            .format = GL_SRGB8_ALPHA8,
            .sampleCount = 1,
            .width = (uint32_t)swapchains[i].width,
            .height = (uint32_t)swapchains[i].height,
            .faceCount = 1,
            .arraySize = 1,
            .mipCount = 1,
        };

        res = xrCreateSwapchain(session, &swapchainInfo, &swapchains[i].handle);
        if (res != XR_SUCCESS) {
            swapchainInfo.format = GL_RGBA8;
            xrCreateSwapchain(session, &swapchainInfo, &swapchains[i].handle);
        }

        uint32_t imageCount = 0;
        xrEnumerateSwapchainImages(swapchains[i].handle, 0, &imageCount, nullptr);
        swapchains[i].images.resize(imageCount, { .type = XR_TYPE_SWAPCHAIN_IMAGE_OPENGL_ES_KHR });
        xrEnumerateSwapchainImages(swapchains[i].handle, imageCount, &imageCount, (XrSwapchainImageBaseHeader*)swapchains[i].images.data());

        swapchains[i].fbos.resize(imageCount);
        swapchains[i].depth_rbs.resize(imageCount);
        for (uint32_t j = 0; j < imageCount; ++j) {
            glGenFramebuffers(1, &swapchains[i].fbos[j]);
            glBindFramebuffer(GL_FRAMEBUFFER, swapchains[i].fbos[j]);
            glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, swapchains[i].images[j].image, 0);

            glGenRenderbuffers(1, &swapchains[i].depth_rbs[j]);
            glBindRenderbuffer(GL_RENDERBUFFER, swapchains[i].depth_rbs[j]);
            glRenderbufferStorage(GL_RENDERBUFFER, GL_DEPTH_COMPONENT24, swapchains[i].width, swapchains[i].height);
            glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_ATTACHMENT, GL_RENDERBUFFER, swapchains[i].depth_rbs[j]);
        }
    }
    glBindFramebuffer(GL_FRAMEBUFFER, 0);

    // Mezcla de entorno Passthrough (Alpha Blend)
    uint32_t blendModeCount = 0;
    xrEnumerateEnvironmentBlendModes(instance, systemId, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, 0, &blendModeCount, nullptr);
    std::vector<XrEnvironmentBlendMode> blendModes(blendModeCount);
    xrEnumerateEnvironmentBlendModes(instance, systemId, XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO, blendModeCount, &blendModeCount, blendModes.data());
    XrEnvironmentBlendMode chosenBlendMode = XR_ENVIRONMENT_BLEND_MODE_OPAQUE;
    for (auto mode : blendModes) {
        if (mode == XR_ENVIRONMENT_BLEND_MODE_ALPHA_BLEND) {
            chosenBlendMode = XR_ENVIRONMENT_BLEND_MODE_ALPHA_BLEND;
        }
    }

    init_tabletop_scene();
    init_audio_system();

    // 9. Bucle de ejecución
    bool sessionRunning = false;
    s_running = true;
    uint64_t frame_index = 0;
    float anim_time = 0.0f;
    bool prev_trigger = false;
    bool prev_grip = false;
    // Marca de tiempo del frame anterior: el delta real alimenta el pan de la
    // vista y las velocidades dependientes del tiempo (cintas, insertadores).
    auto last_frame_time = std::chrono::steady_clock::now();
    float dt_seconds = 0.016f;

    while (s_running) {
        pump_android_input_queue();

        XrEventDataBuffer eventData = { .type = XR_TYPE_EVENT_DATA_BUFFER };
        while (xrPollEvent(instance, &eventData) == XR_SUCCESS) {
            if (eventData.type == XR_TYPE_EVENT_DATA_SESSION_STATE_CHANGED) {
                auto* stateEvent = (XrEventDataSessionStateChanged*)&eventData;
                LOGI("OpenXR SessionStateChanged: estado=%d (actual_running=%d)", (int)stateEvent->state, sessionRunning ? 1 : 0);
                if (stateEvent->state == XR_SESSION_STATE_READY ||
                    ((stateEvent->state == XR_SESSION_STATE_SYNCHRONIZED ||
                      stateEvent->state == XR_SESSION_STATE_VISIBLE ||
                      stateEvent->state == XR_SESSION_STATE_FOCUSED) && !sessionRunning)) {
                    XrSessionBeginInfo beginInfo = {
                        .type = XR_TYPE_SESSION_BEGIN_INFO,
                        .primaryViewConfigurationType = XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO,
                    };
                    XrResult r = xrBeginSession(session, &beginInfo);
                    LOGI("xrBeginSession resultado: %d (0=SUCCESS)", (int)r);
                    if (XR_SUCCEEDED(r)) {
                        sessionRunning = true;
                        LOGI("=== OpenXR SESSION ACTIVA Y RENDERIZANDO ===");
                    }
                } else if (stateEvent->state == XR_SESSION_STATE_STOPPING) {
                    if (sessionRunning) {
                        xrEndSession(session);
                        sessionRunning = false;
                        LOGI("=== OpenXR SESSION DETENIDA (STOPPING) ===");
                    }
                } else if (stateEvent->state == XR_SESSION_STATE_EXITING ||
                           stateEvent->state == XR_SESSION_STATE_LOSS_PENDING) {
                    LOGI("=== OpenXR SESSION SALIENDO (EXITING/LOSS) ===");
                    s_running = false;
                }
            }
            eventData.type = XR_TYPE_EVENT_DATA_BUFFER;
        }

        if (!sessionRunning) {
            usleep(10000);
            continue;
        }

        XrFrameWaitInfo waitInfo = { .type = XR_TYPE_FRAME_WAIT_INFO };
        XrFrameState frameState = { .type = XR_TYPE_FRAME_STATE };
        xrWaitFrame(session, &waitInfo, &frameState);

        // Delta real del frame. Se acota para que una pausa larga del runtime
        // no dispare un salto de animacion ni un pan descontrolado.
        {
            auto now = std::chrono::steady_clock::now();
            dt_seconds = std::chrono::duration<float>(now - last_frame_time).count();
            last_frame_time = now;
            if (!(dt_seconds > 0.0f) || dt_seconds > 0.25f) {
                dt_seconds = 0.016f;
            }
        }

        XrFrameBeginInfo beginInfo = { .type = XR_TYPE_FRAME_BEGIN_INFO };
        xrBeginFrame(session, &beginInfo);

        if (!frameState.shouldRender) {
            XrFrameEndInfo endInfo = {
                .type = XR_TYPE_FRAME_END_INFO,
                .displayTime = frameState.predictedDisplayTime,
                .environmentBlendMode = chosenBlendMode,
                .layerCount = 0,
            };
            xrEndFrame(session, &endInfo);
            continue;
        }

        // Localizar posición de ojos en STAGE
        XrViewLocateInfo locateInfo = {
            .type = XR_TYPE_VIEW_LOCATE_INFO,
            .viewConfigurationType = XR_VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO,
            .displayTime = frameState.predictedDisplayTime,
            .space = appSpace,
        };
        XrViewState viewState = { .type = XR_TYPE_VIEW_STATE };
        std::vector<XrView> views(viewCount, { .type = XR_TYPE_VIEW });
        uint32_t viewCountOut = 0;
        xrLocateViews(session, &locateInfo, &viewState, viewCount, &viewCountOut, views.data());

        // Actualizar posición y orientación del oyente para Audio Espacial 3D
        if (viewState.viewStateFlags & XR_VIEW_STATE_POSITION_VALID_BIT) {
            s_listener_pos.x = (views[0].pose.position.x + views[1].pose.position.x) * 0.5f;
            s_listener_pos.y = (views[0].pose.position.y + views[1].pose.position.y) * 0.5f;
            s_listener_pos.z = (views[0].pose.position.z + views[1].pose.position.z) * 0.5f;

            float fx = 0.0f, fz = -1.0f;
            yawForwardFromQuat(views[0].pose.orientation, &fx, &fz);
            s_listener_yaw = atan2f(fx, -fz);
        }

        // Anclar mesa en el mundo real en el primer frame válido
        if (!s_table_placed && (viewState.viewStateFlags & XR_VIEW_STATE_POSITION_VALID_BIT)) {
            float hx = (views[0].pose.position.x + views[1].pose.position.x) * 0.5f;
            float hy = (views[0].pose.position.y + views[1].pose.position.y) * 0.5f;
            float hz = (views[0].pose.position.z + views[1].pose.position.z) * 0.5f;

            float fx = 0.0f, fz = -1.0f;
            yawForwardFromQuat(views[0].pose.orientation, &fx, &fz);

            float dist = 1.05f;
            s_table_pos.x = hx + fx * dist;
            s_table_pos.z = hz + fz * dist;

            if (hy < 1.35f) {
                s_table_pos.y = (hy - 0.42f > 0.65f) ? (hy - 0.42f) : 0.72f;
            } else {
                s_table_pos.y = 0.80f;
            }

            s_table_yaw = atan2f(-fx, -fz);

            float rot[16], trans[16];
            mat4_rotate_y(rot, s_table_yaw);
            mat4_translate(trans, s_table_pos.x, s_table_pos.y, s_table_pos.z);
            mat4_multiply(trans, rot, s_table_model_mat);

            s_table_placed = true;
            LOGI("=== TABLETOP FIJADO EN STAGE: pos=(%.2f, %.2f, %.2f) yaw=%.2f ===",
                 s_table_pos.x, s_table_pos.y, s_table_pos.z, s_table_yaw);
        }

        // Sincronizar acciones del mando
        XrActiveActionSet activeSet = { .actionSet = actionSet };
        XrActionsSyncInfo syncInfo = {
            .type = XR_TYPE_ACTIONS_SYNC_INFO,
            .countActiveActionSets = 1,
            .activeActionSets = &activeSet,
        };
        xrSyncActions(session, &syncInfo);

        // Consultar puntero 6DoF
        XrSpaceLocation rightAimLoc = { .type = XR_TYPE_SPACE_LOCATION };
        xrLocateSpace(rightAimSpace, appSpace, frameState.predictedDisplayTime, &rightAimLoc);
        bool rightAimValid = (rightAimLoc.locationFlags & XR_SPACE_LOCATION_POSITION_VALID_BIT) &&
                             (rightAimLoc.locationFlags & XR_SPACE_LOCATION_ORIENTATION_VALID_BIT);

        // Consultar botones del mando
        XrActionStateFloat trigState = { .type = XR_TYPE_ACTION_STATE_FLOAT };
        XrActionStateGetInfo getTrig = { .type = XR_TYPE_ACTION_STATE_GET_INFO, .action = rightTrigAction };
        xrGetActionStateFloat(session, &getTrig, &trigState);
        bool trigger_clicked = (trigState.currentState > 0.5f) && !prev_trigger;
        prev_trigger = (trigState.currentState > 0.5f);

        XrActionStateFloat gripState = { .type = XR_TYPE_ACTION_STATE_FLOAT };
        XrActionStateGetInfo getGrip = { .type = XR_TYPE_ACTION_STATE_GET_INFO, .action = rightGripAction };
        xrGetActionStateFloat(session, &getGrip, &gripState);
        bool grip_clicked = (gripState.currentState > 0.5f) && !prev_grip;
        prev_grip = (gripState.currentState > 0.5f);

        XrActionStateVector2f stickState = { .type = XR_TYPE_ACTION_STATE_VECTOR2F };
        XrActionStateGetInfo getStick = { .type = XR_TYPE_ACTION_STATE_GET_INFO, .action = rightStickAction };
        xrGetActionStateVector2f(session, &getStick, &stickState);

        // Desplazamiento de la ventana de mapa con el joystick izquierdo.
        // Permite explorar la fabrica mas alla de las casillas iniciales.
        // El stick derecho queda para la altura de la mesa y el giro de colocacion.
        XrActionStateVector2f leftStickState = { .type = XR_TYPE_ACTION_STATE_VECTOR2F };
        XrActionStateGetInfo getLeftStick = { .type = XR_TYPE_ACTION_STATE_GET_INFO, .action = leftStickAction };
        xrGetActionStateVector2f(session, &getLeftStick, &leftStickState);

        if (s_pan_view) {
            const float deadzone = 0.18f;
            float stick_len = sqrtf(leftStickState.currentState.x * leftStickState.currentState.x +
                                    leftStickState.currentState.y * leftStickState.currentState.y);
            if (stick_len > deadzone) {
                // El desplazamiento se escala por el tiempo real del frame para
                // que la velocidad de exploracion sea igual a 60 y 90 Hz.
                float pan_speed = 9.0f * dt_seconds;
                s_pan_view(leftStickState.currentState.x * pan_speed,
                           leftStickState.currentState.y * pan_speed);
            }
        }

        // El centro de vista vive en Rust (lo usa el radio del snapshot RCON).
        // Se relee para que el render y la consulta al motor nunca divergan.
        if (s_get_view_center) {
            float cx = 0.0f, cy = 0.0f;
            if (s_get_view_center(&cx, &cy)) {
                s_view_pan_x = cx;
                s_view_pan_y = cy;
            }
        }

        XrActionStateFloat leftGripState = { .type = XR_TYPE_ACTION_STATE_FLOAT };
        XrActionStateGetInfo getLeftGrip = { .type = XR_TYPE_ACTION_STATE_GET_INFO, .action = leftGripAction };
        xrGetActionStateFloat(session, &getLeftGrip, &leftGripState);

        XrSpaceLocation leftAimLoc = { .type = XR_TYPE_SPACE_LOCATION };
        xrLocateSpace(leftAimSpace, appSpace, frameState.predictedDisplayTime, &leftAimLoc);
        bool leftAimValid = (leftAimLoc.locationFlags & XR_SPACE_LOCATION_POSITION_VALID_BIT);

        // 1. Interacción Bimanual: Zoom y Rotación con Dos Manos (Generals XR pattern)
        bool both_grips = rightAimValid && leftAimValid && (gripState.currentState > 0.5f) && (leftGripState.currentState > 0.5f);

        if (both_grips) {
            float dx = rightAimLoc.pose.position.x - leftAimLoc.pose.position.x;
            float dy = rightAimLoc.pose.position.y - leftAimLoc.pose.position.y;
            float dz = rightAimLoc.pose.position.z - leftAimLoc.pose.position.z;
            float dist = sqrtf(dx * dx + dy * dy + dz * dz);
            float angle = atan2f(dz, dx);

            if (!s_dual_grip_active) {
                s_dual_grip_active = true;
                s_dual_grip_initial_dist = fmaxf(dist, 0.05f);
                s_dual_grip_initial_scale = s_table_scale;
                s_dual_grip_initial_angle = angle;
                s_dual_grip_initial_yaw = s_table_yaw;
            } else {
                // Escalar (Pinch-to-zoom con las dos manos)
                float ratio = dist / s_dual_grip_initial_dist;
                s_table_scale = fminf(fmaxf(s_dual_grip_initial_scale * ratio, 0.25f), 4.0f);

                // Rotar sobre eje vertical Y con las dos manos
                float delta_angle = angle - s_dual_grip_initial_angle;
                s_table_yaw = s_dual_grip_initial_yaw - delta_angle;

                update_table_matrix();
            }
        } else if (s_dual_grip_active) {
            s_dual_grip_active = false;
            save_tabletop_layout();
        } else if (rightAimValid && gripState.currentState > 0.5f && !grip_clicked) {
            // 2. Agarre y arrastre monomanual de la mesa (XrSurfaceGrab)
            if (!s_table_grabbed) {
                s_table_grabbed = true;
                s_grab_start_hand = rightAimLoc.pose.position;
                s_grab_start_table = s_table_pos;
            } else {
                s_table_pos.x = s_grab_start_table.x + (rightAimLoc.pose.position.x - s_grab_start_hand.x);
                s_table_pos.y = s_grab_start_table.y + (rightAimLoc.pose.position.y - s_grab_start_hand.y);
                s_table_pos.z = s_grab_start_table.z + (rightAimLoc.pose.position.z - s_grab_start_hand.z);

                update_table_matrix();
            }
        } else if (gripState.currentState <= 0.5f) {
            if (s_table_grabbed) {
                s_table_grabbed = false;
                save_tabletop_layout();
            }
        }

        // 2. Rotación de herramienta de colocación con Joystick X o ajuste de altura con Y
        if (!s_table_grabbed && !s_observer_mode) {
            if (fabsf(stickState.currentState.y) > 0.2f) {
                s_table_pos.y += stickState.currentState.y * 0.004f;
                update_table_matrix();
            }
            if (fabsf(stickState.currentState.x) > 0.6f && !s_stick_rotated) {
                s_placement_yaw += (stickState.currentState.x > 0.0f) ? 1.5707963f : -1.5707963f;
                s_stick_rotated = true;
                LOGI("Rotación de colocación 6DoF: yaw=%.2f", s_placement_yaw);
            } else if (fabsf(stickState.currentState.x) < 0.2f) {
                s_stick_rotated = false;
            }
        } else if (s_observer_mode) {
            // Desplazamiento a pie en modo observador humano
            if (fabsf(stickState.currentState.y) > 0.15f || fabsf(stickState.currentState.x) > 0.15f) {
                s_observer_pos.z -= stickState.currentState.y * 0.04f;
                s_observer_pos.x += stickState.currentState.x * 0.04f;
            }
        }

        anim_time += dt_seconds;

        std::vector<XrCompositionLayerProjectionView> projViews(viewCount, { .type = XR_TYPE_COMPOSITION_LAYER_PROJECTION_VIEW });

        for (uint32_t i = 0; i < viewCount; ++i) {
            uint32_t imgIndex = 0;
            XrSwapchainImageAcquireInfo acqInfo = { .type = XR_TYPE_SWAPCHAIN_IMAGE_ACQUIRE_INFO };
            xrAcquireSwapchainImage(swapchains[i].handle, &acqInfo, &imgIndex);

            XrSwapchainImageWaitInfo waitImg = { .type = XR_TYPE_SWAPCHAIN_IMAGE_WAIT_INFO, .timeout = XR_INFINITE_DURATION };
            xrWaitSwapchainImage(swapchains[i].handle, &waitImg);

            glBindFramebuffer(GL_FRAMEBUFFER, swapchains[i].fbos[imgIndex]);
            glViewport(0, 0, swapchains[i].width, swapchains[i].height);

            glClearColor(0.0f, 0.0f, 0.0f, 0.0f);
            glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);

            float proj[16], viewMat[16], vp_mat[16];
            mat4_perspective_openxr(views[i].fov, 0.05f, 60.0f, proj);
            mat4_view_from_pose(views[i].pose, viewMat);
            mat4_multiply(proj, viewMat, vp_mat);

            draw_tabletop_scene(vp_mat, anim_time, dt_seconds, rightAimLoc.pose, rightAimValid, trigger_clicked, (trigState.currentState > 0.5f), grip_clicked, (i == 0), session, rightHapticAction);

            XrSwapchainImageReleaseInfo relInfo = { .type = XR_TYPE_SWAPCHAIN_IMAGE_RELEASE_INFO };
            xrReleaseSwapchainImage(swapchains[i].handle, &relInfo);

            projViews[i].pose = views[i].pose;
            projViews[i].fov = views[i].fov;
            projViews[i].subImage.swapchain = swapchains[i].handle;
            projViews[i].subImage.imageRect.offset = { 0, 0 };
            projViews[i].subImage.imageRect.extent = { swapchains[i].width, swapchains[i].height };
            projViews[i].subImage.imageArrayIndex = 0;
        }

        glBindFramebuffer(GL_FRAMEBUFFER, 0);

        XrCompositionLayerProjection layer = {
            .type = XR_TYPE_COMPOSITION_LAYER_PROJECTION,
            .layerFlags = XR_COMPOSITION_LAYER_BLEND_TEXTURE_SOURCE_ALPHA_BIT
                        | XR_COMPOSITION_LAYER_CORRECT_CHROMATIC_ABERRATION_BIT
                        | XR_COMPOSITION_LAYER_UNPREMULTIPLIED_ALPHA_BIT,
            .space = appSpace,
            .viewCount = viewCount,
            .views = projViews.data(),
        };

        // El fondo de la mesa se compone sobre el passthrough: la capa de
        // passthrough va primero y la de proyección encima.
        // ptLayer debe vivir hasta xrEndFrame: el arreglo guarda su dirección.
        XrCompositionLayerPassthroughFB ptLayer = {
            .type = XR_TYPE_COMPOSITION_LAYER_PASSTHROUGH_FB,
            .next = nullptr,
            .flags = 0,
            .space = appSpace,
            .layerHandle = passthroughLayer,
        };
        std::vector<XrCompositionLayerBaseHeader*> layers;
        if (passthroughLayer != XR_NULL_HANDLE) {
            layers.push_back((XrCompositionLayerBaseHeader*)&ptLayer);
        }
        layers.push_back((XrCompositionLayerBaseHeader*)&layer);

        XrFrameEndInfo endInfo = {
            .type = XR_TYPE_FRAME_END_INFO,
            .displayTime = frameState.predictedDisplayTime,
            .environmentBlendMode = chosenBlendMode,
            .layerCount = (uint32_t)layers.size(),
            .layers = layers.data(),
        };
        xrEndFrame(session, &endInfo);

        frame_index++;
        if (frame_index % 180 == 0) {
            LOGI("Frame %llu: Mesa pos=(%.2f, %.2f, %.2f) scale=%.2f Modo=%s",
                 (unsigned long long)frame_index, s_table_pos.x, s_table_pos.y, s_table_pos.z,
                 s_table_scale, s_observer_mode ? "OBSERVADOR 1:1" : "TABLETOP");
        }
    }

    shutdown_audio_system();

    // El receptor del render del juego se para aqui.
    s_game_corriendo.store(false);
    if (s_game_thread) {
        pthread_join(s_game_thread, nullptr);
        s_game_thread = 0;
    }

    activity->vm->DetachCurrentThread();
    LOGI("=== Hilo OpenXR finalizado ===");
    return nullptr;
}

void start_openxr_thread(ANativeActivity* activity) {
    if (s_running) return;
    s_activity = activity;
    pthread_create(&s_engine_thread, nullptr, openxr_thread_main, activity);
    LOGI("Hilo OpenXR lanzado en segundo plano");
}

void stop_openxr_thread() {
    s_running = false;
    if (s_engine_thread) {
        pthread_join(s_engine_thread, nullptr);
        s_engine_thread = 0;
    }
}
