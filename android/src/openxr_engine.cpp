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
#include <dlfcn.h>

#define TAG "XFactorVR_OpenXR"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, TAG, __VA_ARGS__)

static pthread_t s_engine_thread;
static volatile bool s_running = false;
static ANativeActivity* s_activity = nullptr;

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

static inline void mat4_identity(float* m) {
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

// Sub-panel activo de la consola UI holográfica (0 = Power, 1 = Items, 2 = Tech)
static int s_active_ui_tab = 0;
static int s_hovered_ui_btn = -1;

// Texturas Factorio procedurales
static GLuint s_tex_concrete = 0;
static GLuint s_tex_hazard   = 0;
static GLuint s_tex_belt     = 0;
static GLuint s_tex_soil     = 0;
static GLuint s_tex_steel    = 0;

// Sub-lotes estáticos de la fábrica
static GLuint s_vao_soil = 0, s_vbo_soil = 0;
static int s_count_soil = 0;

static GLuint s_vao_concrete = 0, s_vbo_concrete = 0;
static int s_count_concrete = 0;

static GLuint s_vao_hazard = 0, s_vbo_hazard = 0;
static int s_count_hazard = 0;

static GLuint s_vao_machines = 0, s_vbo_machines = 0;
static int s_count_machines = 0;

static GLuint s_vao_belt = 0, s_vbo_belt = 0;
static int s_count_belt = 0;

static GLuint s_vao_ui = 0, s_vbo_ui = 0;
static int s_count_ui = 0;

// Lote dinámico (Ítems en cinta, inserters, drones, radar, láser y retículo)
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

typedef bool (*pfn_xfactorvr_load_glb)(const char* path, float target_width, DioramaVertex** out_verts, int* out_count);
typedef void (*pfn_xfactorvr_free_glb)(DioramaVertex* verts, int count);

static pfn_xfactorvr_load_glb s_load_glb = nullptr;
static pfn_xfactorvr_free_glb s_free_glb = nullptr;

static void init_glb_loader() {
    if (s_load_glb && s_free_glb) return;
    void* handle = RTLD_DEFAULT;
    s_load_glb = (pfn_xfactorvr_load_glb)dlsym(handle, "xfactorvr_load_glb");
    s_free_glb = (pfn_xfactorvr_free_glb)dlsym(handle, "xfactorvr_free_glb");
    if (!s_load_glb) {
        void* mod_handle = dlopen("libxfactorvr.so", RTLD_NOW | RTLD_GLOBAL);
        if (mod_handle) {
            s_load_glb = (pfn_xfactorvr_load_glb)dlsym(mod_handle, "xfactorvr_load_glb");
            s_free_glb = (pfn_xfactorvr_free_glb)dlsym(mod_handle, "xfactorvr_free_glb");
        }
    }
    if (s_load_glb && s_free_glb) {
        LOGI("openxr_engine: Cargador GLB factorio-3d-mod enlazado exitosamente");
    } else {
        LOGE("openxr_engine: No se pudieron resolver símbolos GLB: %s", dlerror());
    }
}

static bool append_glb_model(std::vector<DioramaVertex>& out_verts,
                             const char* path,
                             float target_width,
                             float pos_x, float pos_y, float pos_z,
                             float yaw_deg = 0.0f) {
    init_glb_loader();
    if (!s_load_glb) return false;

    DioramaVertex* raw_verts = nullptr;
    int count = 0;
    if (!s_load_glb(path, target_width, &raw_verts, &count) || !raw_verts || count <= 0) {
        LOGE("openxr_engine: Fallo al cargar GLB: %s", path);
        return false;
    }

    float rad = yaw_deg * (3.14159265f / 180.0f);
    float cos_y = cosf(rad);
    float sin_y = sinf(rad);

    out_verts.reserve(out_verts.size() + count);
    for (int i = 0; i < count; ++i) {
        DioramaVertex v = raw_verts[i];
        float rx = v.x * cos_y + v.z * sin_y;
        float rz = -v.x * sin_y + v.z * cos_y;
        float rnx = v.nx * cos_y + v.nz * sin_y;
        float rnz = -v.nx * sin_y + v.nz * cos_y;

        v.x = rx + pos_x;
        v.y = v.y + pos_y;
        v.z = rz + pos_z;
        v.nx = rnx;
        v.nz = rnz;
        out_verts.push_back(v);
    }

    if (s_free_glb) {
        s_free_glb(raw_verts, count);
    }
    LOGI("openxr_engine: Cargado GLB %s (%d verts) en pos (%.2f, %.2f, %.2f) yaw=%.1f",
         path, count, pos_x, pos_y, pos_z, yaw_deg);
    return true;
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

    // E) Maquinaria Completa Factorio (Modelos 3D de factorio-3d-mod)
    std::vector<DioramaVertex> machine_verts;

    // 1. Assembling Machine (factorio-3d-mod) en (X = -0.22m, Z = -0.22m)
    float am_x = -0.22f, am_z = -0.22f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/assembling-machine/static.glb", 0.18f, am_x, 0.0f, am_z, 0.0f)) {
        add_box_textured(machine_verts, am_x, 0.045f, am_z, 0.18f, 0.09f, 0.18f, 2.0f, 1.0f, 0.28f, 0.48f, 0.65f, 1.0f);
        add_box_textured(machine_verts, am_x, 0.10f, am_z, 0.14f, 0.03f, 0.14f, 1.0f, 1.0f, 0.20f, 0.25f, 0.30f, 1.0f);
        add_cylinder_textured(machine_verts, am_x - 0.045f, 0.13f, am_z - 0.045f, 0.016f, 0.06f, 0.40f, 0.44f, 0.48f, 1.0f);
        add_cylinder_textured(machine_verts, am_x + 0.045f, 0.14f, am_z - 0.035f, 0.018f, 0.08f, 0.40f, 0.44f, 0.48f, 1.0f);
    }

    // 2. Electric Mining Drill (factorio-3d-mod) en (X = 0.25f, Z = -0.22m)
    float md_x = 0.25f, md_z = -0.22f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/electric-mining-drill/static.glb", 0.16f, md_x, 0.0f, md_z, 0.0f)) {
        add_box_textured(machine_verts, md_x, 0.11f, md_z, 0.14f, 0.06f, 0.14f, 1.0f, 1.0f, 0.15f, 0.60f, 0.55f, 1.0f);
    }

    // 3. Steel Furnace (factorio-3d-mod) en (X = 0.44m, Z = -0.06m)
    float sf_x = 0.44f, sf_z = -0.06f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/steel-furnace/static.glb", 0.13f, sf_x, 0.0f, sf_z, 0.0f)) {
        add_box_textured(machine_verts, sf_x, 0.05f, sf_z, 0.12f, 0.10f, 0.12f, 1.0f, 1.0f, 0.48f, 0.46f, 0.44f, 1.0f);
        add_cylinder_textured(machine_verts, sf_x, 0.12f, sf_z, 0.022f, 0.05f, 0.35f, 0.36f, 0.38f, 1.0f);
    }

    // 4. Radar Station (factorio-3d-mod) en (X = -0.44m, Z = 0.22m)
    float rad_x = -0.44f, rad_z = 0.22f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/radar/static1.glb", 0.14f, rad_x, 0.0f, rad_z, 0.0f)) {
        add_box_textured(machine_verts, rad_x, 0.035f, rad_z, 0.10f, 0.07f, 0.10f, 1.0f, 1.0f, 0.30f, 0.34f, 0.38f, 1.0f);
    }

    // 5. Roboport (factorio-3d-mod) en (X = 0.25f, Z = 0.22m)
    float rb_x = 0.25f, rb_z = 0.22f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/roboport/static.glb", 0.18f, rb_x, 0.0f, rb_z, 0.0f)) {
        add_box_textured(machine_verts, rb_x, 0.05f, rb_z, 0.16f, 0.10f, 0.16f, 1.0f, 1.0f, 0.72f, 0.45f, 0.22f, 1.0f);
    }

    // 6. Gun Turret Base y Torreta Superior (factorio-3d-mod) en (X = -0.44m, Z = -0.22m)
    float gt_x = -0.44f, gt_z = -0.22f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/gun-turret/static1.glb", 0.10f, gt_x, 0.0f, gt_z, 0.0f)) {
        add_cylinder_textured(machine_verts, gt_x, 0.02f, gt_z, 0.045f, 0.04f, 0.40f, 0.44f, 0.46f, 1.0f, 8);
    }
    append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/gun-turret/static2.glb", 0.09f, gt_x, 0.045f, gt_z, 45.0f);

    // 7. Steel Chest (factorio-3d-mod) en (X = -0.42m, Z = -0.06m)
    float sc_x = -0.42f, sc_z = -0.06f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/steel-chest/static.glb", 0.07f, sc_x, 0.0f, sc_z, 0.0f)) {
        add_box_textured(machine_verts, sc_x, 0.035f, sc_z, 0.07f, 0.07f, 0.07f, 1.0f, 1.0f, 0.60f, 0.65f, 0.72f, 1.0f);
    }

    // 8. Postes Eléctricos Medianos y Cableado Catenario
    float p1_x = 0.0f, p1_z = -0.28f;
    float p2_x = 0.0f, p2_z = 0.28f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/medium-electric-pole/static.glb", 0.06f, p1_x, 0.0f, p1_z, 0.0f)) {
        add_cylinder_textured(machine_verts, p1_x, 0.11f, p1_z, 0.008f, 0.22f, 0.45f, 0.36f, 0.28f, 1.0f, 8);
    }
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/STRUCTURES/medium-electric-pole/static.glb", 0.06f, p2_x, 0.0f, p2_z, 0.0f)) {
        add_cylinder_textured(machine_verts, p2_x, 0.11f, p2_z, 0.008f, 0.22f, 0.45f, 0.36f, 0.28f, 1.0f, 8);
    }

    // Tendido de cables eléctricos catenarios 3D
    add_catenary_cable(machine_verts, p1_x - 0.042f, 0.22f, p1_z, p2_x - 0.042f, 0.22f, p2_z, 0.035f, 8, 0.20f, 0.22f, 0.24f, 1.0f);
    add_catenary_cable(machine_verts, p1_x + 0.042f, 0.22f, p1_z, p2_x + 0.042f, 0.22f, p2_z, 0.035f, 8, 0.20f, 0.22f, 0.24f, 1.0f);
    add_catenary_cable(machine_verts, p1_x - 0.042f, 0.22f, p1_z, am_x, 0.11f, am_z, 0.025f, 6, 0.20f, 0.22f, 0.24f, 1.0f);
    add_catenary_cable(machine_verts, p1_x + 0.042f, 0.22f, p1_z, md_x, 0.12f, md_z, 0.025f, 6, 0.20f, 0.22f, 0.24f, 1.0f);

    // 9. Ingeniero Factorio (Avatar 3D oficial de factorio-3d-mod)
    float eng_x = 0.0f, eng_z = 0.0f;
    if (!append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/PLAYER/idle.glb", 0.075f, eng_x, 0.0f, eng_z, 0.0f)) {
        add_box_textured(machine_verts, eng_x, 0.036f, eng_z, 0.026f, 0.028f, 0.018f, 1.0f, 1.0f, 0.90f, 0.50f, 0.12f, 1.0f);
    }

    // 10. Biter Hostil Fuera del Perímetro (factorio-3d-mod)
    append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/ENEMIES/biters/biter_tier1.glb", 0.09f, 0.50f, 0.0f, 0.25f, -120.0f);

    // 11. Árbol Alienígena / Vegetación (factorio-3d-mod)
    append_glb_model(machine_verts, "/sdcard/XFactorVR/models/ENTITIES/FOLIAGE/tree1/static.glb", 0.13f, -0.50f, 0.0f, 0.32f, 30.0f);

    s_vao_machines = create_mesh_vao(machine_verts, s_vbo_machines);
    s_count_machines = (int)machine_verts.size();

    // F) Consola Holográfica Espacial Flotante (Spatial Command UI, Generals XR pattern)
    // Angulada 22° hacia los ojos del jugador
    std::vector<DioramaVertex> ui_verts;
    float ui_w = 0.62f, ui_h = 0.26f;
    float ui_z = -hd - 0.08f;
    float ui_y = 0.17f;

    // Panel de cristal oscuro translúcido (#0e141a, alpha 0.92)
    add_box_textured(ui_verts, 0.0f, ui_y, ui_z, ui_w, ui_h, 0.005f, 1.0f, 1.0f, 0.07f, 0.10f, 0.14f, 0.94f);
    // Barra de título superior naranja Factorio
    add_box_textured(ui_verts, 0.0f, ui_y + ui_h * 0.42f, ui_z + 0.004f, ui_w * 0.95f, 0.026f, 0.002f, 1.0f, 1.0f, 1.0f, 0.58f, 0.12f, 1.0f);

    // 5 Botones de comando Factorio:
    // [0: OBSERVER MODE] [1: RECENTER] [2: ENERGÍA] [3: ÍTEMS] [4: TECNOLOGÍA]
    float btn_w = 0.108f, btn_h = 0.034f;
    for (int b = -2; b <= 2; ++b) {
        float bx = 0.0f + (float)b * (btn_w + 0.012f);
        float br = (b == -2) ? 0.85f : 0.18f;
        float bg = (b == -2) ? 0.35f : 0.52f;
        float bb = (b == -2) ? 0.20f : 0.70f;
        add_box_textured(ui_verts, bx, ui_y + 0.045f, ui_z + 0.004f, btn_w, btn_h, 0.003f, 1.0f, 1.0f, br, bg, bb, 1.0f);
    }

    // Área de datos dinámicos holográficos del sub-panel
    add_box_textured(ui_verts, 0.0f, ui_y - 0.045f, ui_z + 0.003f, ui_w * 0.92f, 0.10f, 0.002f, 1.0f, 1.0f, 0.12f, 0.16f, 0.22f, 0.85f);
    // Barra de progreso de ensamblaje / investigación
    add_box_textured(ui_verts, 0.0f, ui_y - 0.075f, ui_z + 0.005f, ui_w * 0.85f, 0.015f, 0.002f, 1.0f, 1.0f, 0.20f, 0.75f, 0.95f, 1.0f);

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

static void draw_tabletop_scene(const float* vp_mat,
                                float anim_time,
                                const XrPosef& right_aim_pose,
                                bool right_aim_valid,
                                bool trigger_clicked) {
    if (s_program == 0) return;

    glUseProgram(s_program);
    glEnable(GL_DEPTH_TEST);
    glDepthFunc(GL_LEQUAL);
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);

    // Matriz de modelo activa (Diorama de mesa o Modo Observador a escala 1:1)
    float active_model[16];
    if (s_observer_mode) {
        // En modo observador, escalamos 25x y centramos en s_observer_pos
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
    // En modo observador se desactiva el recorte para ver el horizonte de la fábrica
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

    // E) Maquinaria y estructuras estáticas (Modelos 3D de factorio-3d-mod)
    glUniform1f(s_use_tex_loc, 0.0f); // Usar colores de textura 3D originales horneados por vértice
    glUniform1f(s_lit_loc, 1.0f);
    glBindVertexArray(s_vao_machines);
    glDrawArrays(GL_TRIANGLES, 0, s_count_machines);

    // F) Consola UI Holográfica Espacial (Generals XR pattern)
    if (!s_observer_mode) {
        glUniform1f(s_enable_clipping_loc, 0.0f); // Sin recorte para el panel flotante en el aire
        glUniform1f(s_use_tex_loc, 0.0f);
        glUniform1f(s_lit_loc, 0.35f);
        glBindVertexArray(s_vao_ui);
        glDrawArrays(GL_TRIANGLES, 0, s_count_ui);
        glUniform1f(s_enable_clipping_loc, 1.0f); // Restaurar recorte de la mesa física
    }

    // 2. Elementos dinámicos animados de la maqueta
    std::vector<DioramaVertex> dyn_verts;

    // A) Inserter 1 (Amarillo estándar) alimentando Assembling Machine 2
    float ins1_x = -0.08f, ins1_z = -0.13f;
    float ins1_yaw = sinf(anim_time * 2.2f) * 0.82f;
    add_cylinder_textured(dyn_verts, ins1_x, 0.02f, ins1_z, 0.025f, 0.04f, 0.95f, 0.68f, 0.12f, 1.0f, 8);
    float arm1_len = 0.07f;
    float arm1_pitch = 0.42f + sinf(anim_time * 2.2f) * 0.14f;
    float elb1_x = ins1_x + sinf(ins1_yaw) * cosf(arm1_pitch) * arm1_len;
    float elb1_y = 0.04f + sinf(arm1_pitch) * arm1_len;
    float elb1_z = ins1_z + cosf(ins1_yaw) * cosf(arm1_pitch) * arm1_len;
    add_box_textured(dyn_verts, (ins1_x + elb1_x) * 0.5f, (0.04f + elb1_y) * 0.5f, (ins1_z + elb1_z) * 0.5f,
                     0.012f, 0.012f, arm1_len, 1.0f, 1.0f, 0.28f, 0.32f, 0.36f, 1.0f);
    add_cylinder_textured(dyn_verts, elb1_x, elb1_y, elb1_z, 0.014f, 0.02f, 0.95f, 0.68f, 0.12f, 1.0f, 8);
    float claw1_x = elb1_x + sinf(ins1_yaw) * 0.045f;
    float claw1_y = elb1_y - 0.030f;
    float claw1_z = elb1_z + cosf(ins1_yaw) * 0.045f;
    add_box_textured(dyn_verts, (elb1_x + claw1_x) * 0.5f, (elb1_y + claw1_y) * 0.5f, (elb1_z + claw1_z) * 0.5f,
                     0.010f, 0.010f, 0.06f, 1.0f, 1.0f, 0.28f, 0.32f, 0.36f, 1.0f);
    add_box_textured(dyn_verts, claw1_x - 0.008f, claw1_y - 0.01f, claw1_z, 0.004f, 0.016f, 0.014f, 1.0f, 1.0f, 0.8f, 0.8f, 0.85f, 1.0f);
    add_box_textured(dyn_verts, claw1_x + 0.008f, claw1_y - 0.01f, claw1_z, 0.004f, 0.016f, 0.014f, 1.0f, 1.0f, 0.8f, 0.8f, 0.85f, 1.0f);

    // B) Inserter 2 (Azul rápido) extrayendo circuitos hacia cinta 2
    float ins2_x = -0.22f, ins2_z = -0.06f;
    float ins2_yaw = -cosf(anim_time * 3.4f) * 0.85f;
    add_cylinder_textured(dyn_verts, ins2_x, 0.02f, ins2_z, 0.025f, 0.04f, 0.20f, 0.55f, 0.95f, 1.0f, 8);
    float elb2_x = ins2_x + sinf(ins2_yaw) * 0.06f;
    float elb2_y = 0.055f;
    float elb2_z = ins2_z + cosf(ins2_yaw) * 0.06f;
    add_box_textured(dyn_verts, (ins2_x + elb2_x) * 0.5f, (0.04f + elb2_y) * 0.5f, (ins2_z + elb2_z) * 0.5f,
                     0.012f, 0.012f, 0.065f, 1.0f, 1.0f, 0.22f, 0.26f, 0.30f, 1.0f);
    add_cylinder_textured(dyn_verts, elb2_x, elb2_y, elb2_z, 0.014f, 0.02f, 0.20f, 0.55f, 0.95f, 1.0f, 8);

    // C) Antena Parabólica de Radar Giratoria (360° continuo)
    float rad_x = -0.44f, rad_z = 0.22f;
    float rad_ang = anim_time * 1.5f;
    float dish_dx = sinf(rad_ang) * 0.04f;
    float dish_dz = cosf(rad_ang) * 0.04f;
    add_box_textured(dyn_verts, rad_x, 0.13f, rad_z, 0.08f, 0.018f, 0.025f, 1.0f, 1.0f, 0.82f, 0.85f, 0.88f, 1.0f);
    add_cylinder_textured(dyn_verts, rad_x + dish_dx, 0.13f, rad_z + dish_dz, 0.035f, 0.010f, 0.92f, 0.94f, 0.96f, 1.0f, 10);

    // D) Drones Logísticos en patrulla aérea sobre el Roboport
    float rb_x = 0.25f, rb_z = 0.22f;
    for (int d = 0; d < 2; ++d) {
        float d_ang = anim_time * 1.8f + (float)d * 3.14159f;
        float drone_x = rb_x + sinf(d_ang) * 0.08f;
        float drone_z = rb_z + sinf(d_ang * 2.0f) * 0.05f;
        float drone_y = 0.18f + cosf(d_ang) * 0.02f;
        // Chasis del dron
        add_box_textured(dyn_verts, drone_x, drone_y, drone_z, 0.024f, 0.010f, 0.024f, 1.0f, 1.0f, 0.90f, 0.92f, 0.95f, 1.0f);
        // 4 Rotores
        add_cylinder_textured(dyn_verts, drone_x - 0.014f, drone_y + 0.006f, drone_z - 0.014f, 0.008f, 0.002f, 0.2f, 0.2f, 0.2f, 0.8f, 6);
        add_cylinder_textured(dyn_verts, drone_x + 0.014f, drone_y + 0.006f, drone_z - 0.014f, 0.008f, 0.002f, 0.2f, 0.2f, 0.2f, 0.8f, 6);
        add_cylinder_textured(dyn_verts, drone_x - 0.014f, drone_y + 0.006f, drone_z + 0.014f, 0.008f, 0.002f, 0.2f, 0.2f, 0.2f, 0.8f, 6);
        add_cylinder_textured(dyn_verts, drone_x + 0.014f, drone_y + 0.006f, drone_z + 0.014f, 0.008f, 0.002f, 0.2f, 0.2f, 0.2f, 0.8f, 6);
    }

    // E) Cañón de la Torreta Defensiva girando en guardia
    float gt_x = -0.44f, gt_z = -0.22f;
    float gt_yaw = sinf(anim_time * 0.8f) * 0.65f;
    float b_dx = sinf(gt_yaw) * 0.04f, b_dz = cosf(gt_yaw) * 0.04f;
    add_box_textured(dyn_verts, gt_x + b_dx * 0.5f - 0.006f, 0.065f, gt_z + b_dz * 0.5f, 0.006f, 0.006f, 0.045f, 1.0f, 1.0f, 0.2f, 0.22f, 0.25f, 1.0f);
    add_box_textured(dyn_verts, gt_x + b_dx * 0.5f + 0.006f, 0.065f, gt_z + b_dz * 0.5f, 0.006f, 0.006f, 0.045f, 1.0f, 1.0f, 0.2f, 0.22f, 0.25f, 1.0f);

    // F) Vástago giratorio de la perforadora eléctrica
    float md_x = 0.25f, md_z = -0.22f;
    add_cylinder_textured(dyn_verts, md_x, 0.05f, md_z, 0.024f, 0.08f, 0.55f, 0.60f, 0.65f, 1.0f, 8);

    // G) Ítems en movimiento fluido sobre las cintas
    float item_span = TABLE_HALF_W * 1.55f;
    for (int k = 0; k < 7; ++k) {
        float offset = fmodf(anim_time * 0.16f + (float)k * (item_span / 7.0f), item_span) - item_span * 0.5f;
        // Placas de hierro en carril 1
        add_box_textured(dyn_verts, offset, 0.016f, -0.06f, 0.030f, 0.005f, 0.030f, 1.0f, 1.0f, 0.68f, 0.74f, 0.80f, 1.0f);
        // Circuitos electrónicos verdes en carril 2
        add_box_textured(dyn_verts, -offset, 0.016f, 0.06f, 0.026f, 0.004f, 0.026f, 1.0f, 1.0f, 0.16f, 0.82f, 0.35f, 1.0f);
        // Engranajes de hierro rotando sobre la cinta
        if (k % 2 == 0) {
            add_cylinder_textured(dyn_verts, offset * 0.7f, 0.018f, -0.06f, 0.012f, 0.004f, 0.85f, 0.72f, 0.25f, 1.0f, 8);
        }
    }

    // H) Baliza titilante en la chimenea de la ensambladora
    float beacon_r = 1.0f, beacon_g = 0.58f + sinf(anim_time * 5.0f) * 0.38f, beacon_b = 0.1f;
    add_cylinder_textured(dyn_verts, -0.22f + 0.065f, 0.12f, -0.22f + 0.065f, 0.008f, 0.016f, beacon_r, beacon_g, beacon_b, 1.0f, 6);

    // I) Vapor de chimenea en partículas ascendentes
    float puff_y = 0.18f + fmodf(anim_time * 0.08f, 0.06f);
    float puff_a = 0.45f * (1.0f - (puff_y - 0.18f) / 0.06f);
    add_cylinder_textured(dyn_verts, -0.22f - 0.045f, puff_y, -0.22f - 0.045f, 0.014f, 0.015f, 0.90f, 0.92f, 0.95f, puff_a, 6);

    if (!dyn_verts.empty()) {
        glUniform1f(s_use_tex_loc, 0.0f);
        glBindVertexArray(s_dynamic_vao);
        glBindBuffer(GL_ARRAY_BUFFER, s_dynamic_vbo);
        glBufferSubData(GL_ARRAY_BUFFER, 0, dyn_verts.size() * sizeof(DioramaVertex), dyn_verts.data());

        glUniform1f(s_lit_loc, 1.0f);
        glDrawArrays(GL_TRIANGLES, 0, (GLsizei)dyn_verts.size());
    }

    // ========================================================================
    // 3. RAYO LÁSER 6DoF, RETÍCULO Y CLIC EN CONSOLA UI
    // ========================================================================
    if (right_aim_valid) {
        float ident_mat[16];
        matIdentity(ident_mat);
        glUniformMatrix4fv(s_model_loc, 1, GL_FALSE, ident_mat);
        glUniformMatrix4fv(s_mvp_loc, 1, GL_FALSE, vp_mat);
        glUniform1f(s_enable_clipping_loc, 0.0f);
        glUniform1f(s_lit_loc, 0.0f);
        glUniform3f(s_emissive_loc, 0.2f, 0.8f, 1.0f);

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
        bool has_table_hit = false;
        bool has_ui_hit = false;
        float ray_len = 1.6f;

        // A) Raycast contra el plano físico de la mesa en STAGE (Y = s_table_pos.y)
        if (fabsf(dy) > 1e-4f) {
            float t = (s_table_pos.y - ry) / dy;
            if (t > 0.0f && t < 4.0f) {
                float cand_x = rx + dx * t;
                float cand_z = rz + dz * t;

                float rel_x = cand_x - s_table_pos.x;
                float rel_z = cand_z - s_table_pos.z;
                float cos_y = cosf(-s_table_yaw);
                float sin_y = sinf(-s_table_yaw);
                float loc_x = (rel_x * cos_y - rel_z * sin_y) / s_table_scale;
                float loc_z = (rel_x * sin_y + rel_z * cos_y) / s_table_scale;

                if (fabsf(loc_x) <= TABLE_HALF_W && fabsf(loc_z) <= TABLE_HALF_D) {
                    has_table_hit = true;
                    ray_len = t;
                    hit_stage_x = cand_x;
                    hit_stage_z = cand_z;
                }
            }
        }

        // B) Raycast contra la Consola UI Holográfica y botones
        s_hovered_ui_btn = -1;
        float ui_plane_z = -TABLE_HALF_D - 0.08f;
        float ui_world_z = s_table_pos.z + (ui_plane_z * cosf(s_table_yaw));
        float t_ui = (ui_world_z - rz) / (dz != 0 ? dz : 1e-4f);
        if (t_ui > 0.0f && t_ui < ray_len) {
            float ui_hit_x = rx + dx * t_ui;
            float ui_hit_y = ry + dy * t_ui;
            float rel_x = ui_hit_x - s_table_pos.x;
            float rel_y = ui_hit_y - (s_table_pos.y + 0.17f);
            if (fabsf(rel_x) < 0.31f && fabsf(rel_y) < 0.13f) {
                has_ui_hit = true;
                ray_len = t_ui;

                // Identificar qué botón está apuntado
                float btn_w = 0.108f;
                for (int b = -2; b <= 2; ++b) {
                    float bx = (float)b * (btn_w + 0.012f);
                    if (fabsf(rel_x - bx) < btn_w * 0.5f && fabsf(rel_y - 0.045f) < 0.018f) {
                        s_hovered_ui_btn = b + 2; // 0..4
                        break;
                    }
                }

                // Clic en botón de la interfaz con el gatillo Trigger
                if (trigger_clicked && s_hovered_ui_btn >= 0) {
                    if (s_hovered_ui_btn == 0) {
                        // Alternar Modo Observador a escala 1:1 vs Modo Diorama Tabletop
                        s_observer_mode = !s_observer_mode;
                        LOGI("Modo cambiado: %s", s_observer_mode ? "OBSERVADOR 1:1" : "TABLETOP DIORAMA");
                    } else if (s_hovered_ui_btn == 1) {
                        // Re-anclar mesa frente al usuario
                        s_table_placed = false;
                        LOGI("Re-anclando mesa...");
                    } else {
                        // Cambiar sub-panel activo
                        s_active_ui_tab = s_hovered_ui_btn - 2;
                        LOGI("Sub-panel activo: %d", s_active_ui_tab);
                    }
                }
            }
        }

        std::vector<DioramaVertex> laser_verts;
        float end_x = rx + dx * ray_len;
        float end_y = ry + dy * ray_len;
        float end_z = rz + dz * ray_len;
        float l_r = (has_ui_hit || has_table_hit) ? 0.15f : 0.85f;
        float l_g = (has_ui_hit || has_table_hit) ? 0.95f : 0.35f;
        float l_b = (has_ui_hit || has_table_hit) ? 1.00f : 0.20f;

        add_box_textured(laser_verts, (rx + end_x) * 0.5f, (ry + end_y) * 0.5f, (rz + end_z) * 0.5f,
                         0.003f, 0.003f, ray_len, 1.0f, 1.0f, l_r, l_g, l_b, 0.92f);

        // Retículo pulsante en la mesa
        if (has_table_hit) {
            float ret_rad = 0.032f + sinf(anim_time * 6.5f) * 0.004f;
            for (int s = 0; s < 16; ++s) {
                float a0 = (float)s * 6.2831853f / 16.0f;
                float a1 = (float)(s + 1) * 6.2831853f / 16.0f;
                laser_verts.push_back({hit_stage_x + cosf(a0) * ret_rad, s_table_pos.y + 0.003f, hit_stage_z + sinf(a0) * ret_rad, 0, 1, 0, 0, 0, 0.2f, 0.95f, 1.0f, 0.95f});
                laser_verts.push_back({hit_stage_x + cosf(a1) * ret_rad, s_table_pos.y + 0.003f, hit_stage_z + sinf(a1) * ret_rad, 0, 1, 0, 0, 0, 0.2f, 0.95f, 1.0f, 0.95f});
            }
        }

        if (!laser_verts.empty()) {
            glBindVertexArray(s_dynamic_vao);
            glBindBuffer(GL_ARRAY_BUFFER, s_dynamic_vbo);
            glBufferSubData(GL_ARRAY_BUFFER, 0, laser_verts.size() * sizeof(DioramaVertex), laser_verts.data());
            glDrawArrays(GL_TRIANGLES, 0, (GLsizei)laser_verts.size());
        }
    }

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
    const char* const enabledExtensions[] = {
        "XR_KHR_android_create_instance",
        "XR_KHR_opengl_es_enable",
    };

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
        .enabledExtensionCount = 2,
        .enabledExtensionNames = enabledExtensions,
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

    // Trigger
    XrActionCreateInfo trigInfo = { .type = XR_TYPE_ACTION_CREATE_INFO, .actionType = XR_ACTION_TYPE_FLOAT_INPUT };
    strncpy(trigInfo.actionName, "right_trigger", XR_MAX_ACTION_NAME_SIZE);
    strncpy(trigInfo.localizedActionName, "Right Trigger", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction rightTrigAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &trigInfo, &rightTrigAction);

    // Grip (para agarrar y mover la mesa)
    XrActionCreateInfo gripInfo = { .type = XR_TYPE_ACTION_CREATE_INFO, .actionType = XR_ACTION_TYPE_FLOAT_INPUT };
    strncpy(gripInfo.actionName, "right_grip", XR_MAX_ACTION_NAME_SIZE);
    strncpy(gripInfo.localizedActionName, "Right Grip", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction rightGripAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &gripInfo, &rightGripAction);

    // Thumbstick (para ajustar altura o caminar en modo observador)
    XrActionCreateInfo stickInfo = { .type = XR_TYPE_ACTION_CREATE_INFO, .actionType = XR_ACTION_TYPE_VECTOR2F_INPUT };
    strncpy(stickInfo.actionName, "right_stick", XR_MAX_ACTION_NAME_SIZE);
    strncpy(stickInfo.localizedActionName, "Right Stick", XR_MAX_LOCALIZED_ACTION_NAME_SIZE);
    XrAction rightStickAction = XR_NULL_HANDLE;
    xrCreateAction(actionSet, &stickInfo, &rightStickAction);

    // Bindings de interacción
    XrPath simpleProfile = XR_NULL_PATH, oculusProfile = XR_NULL_PATH, picoProfile = XR_NULL_PATH;
    xrStringToPath(instance, "/interaction_profiles/khr/simple_controller", &simpleProfile);
    xrStringToPath(instance, "/interaction_profiles/oculus/touch_controller", &oculusProfile);
    xrStringToPath(instance, "/interaction_profiles/bytedance/pico4_controller", &picoProfile);

    XrPath rightAimPath = XR_NULL_PATH, rightTrigPath = XR_NULL_PATH, rightGripPath = XR_NULL_PATH, rightStickPath = XR_NULL_PATH;
    xrStringToPath(instance, "/user/hand/right/input/aim/pose", &rightAimPath);
    xrStringToPath(instance, "/user/hand/right/input/trigger/value", &rightTrigPath);
    xrStringToPath(instance, "/user/hand/right/input/squeeze/value", &rightGripPath);
    xrStringToPath(instance, "/user/hand/right/input/thumbstick", &rightStickPath);

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
        { rightStickAction, rightStickPath }
    };
    XrInteractionProfileSuggestedBinding touchSuggested = {
        .type = XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING,
        .interactionProfile = oculusProfile,
        .countSuggestedBindings = 4,
        .suggestedBindings = touchBindings,
    };
    xrSuggestInteractionProfileBindings(instance, &touchSuggested);

    XrInteractionProfileSuggestedBinding picoSuggested = {
        .type = XR_TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING,
        .interactionProfile = picoProfile,
        .countSuggestedBindings = 4,
        .suggestedBindings = touchBindings,
    };
    xrSuggestInteractionProfileBindings(instance, &picoSuggested);

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

    // 9. Bucle de ejecución
    bool sessionRunning = false;
    s_running = true;
    uint64_t frame_index = 0;
    float anim_time = 0.0f;
    bool prev_trigger = false;

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

        XrActionStateVector2f stickState = { .type = XR_TYPE_ACTION_STATE_VECTOR2F };
        XrActionStateGetInfo getStick = { .type = XR_TYPE_ACTION_STATE_GET_INFO, .action = rightStickAction };
        xrGetActionStateVector2f(session, &getStick, &stickState);

        // 1. Agarre y arrastre de la mesa con el gatillo Grip (XrSurfaceGrab)
        if (rightAimValid && gripState.currentState > 0.5f) {
            if (!s_table_grabbed) {
                s_table_grabbed = true;
                s_grab_start_hand = rightAimLoc.pose.position;
                s_grab_start_table = s_table_pos;
            } else {
                s_table_pos.x = s_grab_start_table.x + (rightAimLoc.pose.position.x - s_grab_start_hand.x);
                s_table_pos.y = s_grab_start_table.y + (rightAimLoc.pose.position.y - s_grab_start_hand.y);
                s_table_pos.z = s_grab_start_table.z + (rightAimLoc.pose.position.z - s_grab_start_hand.z);

                float rot[16], trans[16];
                mat4_rotate_y(rot, s_table_yaw);
                mat4_translate(trans, s_table_pos.x, s_table_pos.y, s_table_pos.z);
                mat4_multiply(trans, rot, s_table_model_mat);
            }
        } else {
            s_table_grabbed = false;
        }

        // 2. Ajuste fino de altura y orientación con el Joystick cuando no se arrastra
        if (!s_table_grabbed && !s_observer_mode) {
            if (fabsf(stickState.currentState.y) > 0.2f) {
                s_table_pos.y += stickState.currentState.y * 0.004f;
                float rot[16], trans[16];
                mat4_rotate_y(rot, s_table_yaw);
                mat4_translate(trans, s_table_pos.x, s_table_pos.y, s_table_pos.z);
                mat4_multiply(trans, rot, s_table_model_mat);
            }
            if (fabsf(stickState.currentState.x) > 0.2f) {
                s_table_yaw -= stickState.currentState.x * 0.02f;
                float rot[16], trans[16];
                mat4_rotate_y(rot, s_table_yaw);
                mat4_translate(trans, s_table_pos.x, s_table_pos.y, s_table_pos.z);
                mat4_multiply(trans, rot, s_table_model_mat);
            }
        } else if (s_observer_mode) {
            // Desplazamiento a pie en modo observador humano
            if (fabsf(stickState.currentState.y) > 0.15f || fabsf(stickState.currentState.x) > 0.15f) {
                s_observer_pos.z -= stickState.currentState.y * 0.04f;
                s_observer_pos.x += stickState.currentState.x * 0.04f;
            }
        }

        anim_time += 0.016f;

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

            draw_tabletop_scene(vp_mat, anim_time, rightAimLoc.pose, rightAimValid, trigger_clicked);

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
            .layerFlags = XR_COMPOSITION_LAYER_BLEND_TEXTURE_SOURCE_ALPHA_BIT,
            .space = appSpace,
            .viewCount = viewCount,
            .views = projViews.data(),
        };
        const XrCompositionLayerBaseHeader* layers[] = { (XrCompositionLayerBaseHeader*)&layer };

        XrFrameEndInfo endInfo = {
            .type = XR_TYPE_FRAME_END_INFO,
            .displayTime = frameState.predictedDisplayTime,
            .environmentBlendMode = chosenBlendMode,
            .layerCount = 1u,
            .layers = layers,
        };
        xrEndFrame(session, &endInfo);

        frame_index++;
        if (frame_index % 180 == 0) {
            LOGI("Frame %llu: Mesa pos=(%.2f, %.2f, %.2f) scale=%.2f Modo=%s",
                 (unsigned long long)frame_index, s_table_pos.x, s_table_pos.y, s_table_pos.z,
                 s_table_scale, s_observer_mode ? "OBSERVADOR 1:1" : "TABLETOP");
        }
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
