// ancla_espacial.cpp — Implementación del anclaje espacial a la mesa real
// ============================================================================
//
// Módulo autónomo (namespace xfv). Detecta la superficie horizontal de la mesa
// con las extensiones de sensing espacial de ByteDance (XR_BD_spatial_sensing
// + XR_BD_spatial_anchor + XR_BD_spatial_plane / XR_BD_spatial_scene) que
// expone el runtime OpenXR 1.1 de PICO OS 5.12+, crea un ancla sobre esa
// superficie y devuelve la matriz (column-major) que lleva el diorama de
// Factorio TUMBADO a la mesa física.
//
// ---------------------------------------------------------------------------
// NOTA SOBRE LOS HEADERS
//
// El openxr.h que hay vendorizado en android/include/openxr es el 1.1.45 de
// Khronos y NO incluye las extensiones XR_BD_spatial_*. La definicion
// oficial de esas extensiones vive en el registro de Khronos
// (specification/registry/xr.xml, extensiones 390/391/393/397) y se puede
// generar con el generador oficial (scripts/genxr.py). Para no depender de
// regenerar los headers del repo (y para no tocar ficheros que no son
// nuestros), aqui transcribimos la ABI minima que necesitamos, con los
// MISMOS valores numericos de XrStructureType y las MISMAS disposiciones de
// memoria que produce el generador de Khronos (ver ADR-014).
//
// Si algun dia el repo actualiza sus headers, este bloque debe Poderse borrar
// sin tocar el resto del modulo.
//
// ---------------------------------------------------------------------------
// DEGRADACION
//
// Cualquier fallo (extension ausente, no habilitada, servicio de sensing
// parado, mesa no detectada, tracking perdido) deja el modulo en un estado
// sin ancla. La matriz sigue siendo SIEMPRE la identidad, de modo que el
// visor puede seguir con su ruta STAGE habitual sin romperse.
//
// ============================================================================

#include "ancla_espacial.h"

#include <android/log.h>

#include <unistd.h>

#include <cmath>
#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

// ============================================================================
// BLOQUE ABI: EXTENSIONES XR_BD_spatial_* (no presentes en los headers)
// ============================================================================

namespace xfv {
namespace bd {

// Handles: en OpenXR 64-bit son punteros a estructuras opacas.
typedef struct XfvSenseDataProviderBD_T* XfvSenseDataProviderBD;
typedef struct XfvSenseDataSnapshotBD_T* XfvSenseDataSnapshotBD;
typedef struct XfvAnchorBD_T* XfvAnchorBD;

// XR_DEFINE_ATOM(XrSpatialEntityIdBD) -> uint64_t
typedef uint64_t XfvSpatialEntityIdBD;

// --- XrStructureType (valores exactos del header generado por Khronos) ------
[[maybe_unused]] constexpr XrStructureType kTypeSystemSpatialSensingProperties = (XrStructureType)1000389000;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialEntityComponentGetInfo = (XrStructureType)1000389001;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialEntityLocationGetInfo = (XrStructureType)1000389002;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialEntityComponentDataLocation = (XrStructureType)1000389003;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialEntityComponentDataBoundingBox2D = (XrStructureType)1000389005;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialEntityComponentDataPolygon = (XrStructureType)1000389006;
[[maybe_unused]] constexpr XrStructureType kTypeSenseDataProviderCreateInfo = (XrStructureType)1000389009;
[[maybe_unused]] constexpr XrStructureType kTypeSenseDataProviderStartInfo = (XrStructureType)1000389010;
[[maybe_unused]] constexpr XrStructureType kTypeSenseDataQueryInfo = (XrStructureType)1000389013;
[[maybe_unused]] constexpr XrStructureType kTypeSenseDataQueryCompletion = (XrStructureType)1000389014;
[[maybe_unused]] constexpr XrStructureType kTypeSenseDataFilterUuid = (XrStructureType)1000389015;
[[maybe_unused]] constexpr XrStructureType kTypeQueriedSenseDataGetInfo = (XrStructureType)1000389017;
[[maybe_unused]] constexpr XrStructureType kTypeQueriedSenseData = (XrStructureType)1000389018;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialEntityState = (XrStructureType)1000389019;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialEntityAnchorCreateInfo = (XrStructureType)1000389020;
[[maybe_unused]] constexpr XrStructureType kTypeAnchorSpaceCreateInfo = (XrStructureType)1000389021;
[[maybe_unused]] constexpr XrStructureType kTypeSystemSpatialAnchorProperties = (XrStructureType)1000390000;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialAnchorCreateInfo = (XrStructureType)1000390001;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialAnchorCreateCompletion = (XrStructureType)1000390002;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialAnchorPersistInfo = (XrStructureType)1000390003;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialAnchorUnpersistInfo = (XrStructureType)1000390004;
[[maybe_unused]] constexpr XrStructureType kTypeSystemSpatialPlaneProperties = (XrStructureType)1000396000;
[[maybe_unused]] constexpr XrStructureType kTypeSpatialEntityComponentDataPlaneOrientation =
    (XrStructureType)1000396001;
[[maybe_unused]] constexpr XrStructureType kTypeSenseDataFilterPlaneOrientation = (XrStructureType)1000396002;
[[maybe_unused]] constexpr XrStructureType kTypeFutureCompletionExt = (XrStructureType)1000469002;

// --- Errores ---------------------------------------------------------------
[[maybe_unused]] constexpr XrResult kErrorSpatialEntityIdInvalid = (XrResult)(-1000389000);
[[maybe_unused]] constexpr XrResult kErrorSpatialSensingServiceUnavailable = (XrResult)(-1000389001);
[[maybe_unused]] constexpr XrResult kErrorAnchorNotSupportedForEntity = (XrResult)(-1000389002);
[[maybe_unused]] constexpr XrResult kErrorSpatialAnchorNotFound = (XrResult)(-1000390000);
[[maybe_unused]] constexpr XrResult kErrorFuturePendingExt = (XrResult)(-1000469001);
[[maybe_unused]] constexpr XrResult kErrorFutureInvalidExt = (XrResult)(-1000469002);

// --- Enumeraciones ---------------------------------------------------------
// XrSenseDataProviderTypeBD
[[maybe_unused]] constexpr uint32_t kProviderTypeAnchor = 1000390000u;
[[maybe_unused]] constexpr uint32_t kProviderTypeScene = 1000392000u;
[[maybe_unused]] constexpr uint32_t kProviderTypeMesh = 1000393000u;
[[maybe_unused]] constexpr uint32_t kProviderTypePlane = 1000396000u;

// XrSpatialEntityComponentTypeBD
[[maybe_unused]] constexpr uint32_t kComponentTypeLocation = 0u;
[[maybe_unused]] constexpr uint32_t kComponentTypeSemantic = 1u;
[[maybe_unused]] constexpr uint32_t kComponentTypeBoundingBox2D = 2u;
[[maybe_unused]] constexpr uint32_t kComponentTypePolygon = 3u;
[[maybe_unused]] constexpr uint32_t kComponentTypePlaneOrientation = 1000396000u;

// XrPlaneOrientationBD
[[maybe_unused]] constexpr uint32_t kPlaneOrientationHorizontalUpward = 0u;
[[maybe_unused]] constexpr uint32_t kPlaneOrientationHorizontalDownward = 1u;
[[maybe_unused]] constexpr uint32_t kPlaneOrientationVertical = 2u;
[[maybe_unused]] constexpr uint32_t kPlaneOrientationArbitrary = 3u;

// XrPersistenceLocationBD
[[maybe_unused]] constexpr uint32_t kPersistenceLocationLocal = 0u;

// --- Estructuras (disposición idéntica al header oficial) ------------------

struct XfvSenseDataProviderCreateInfo {
    XrStructureType type;
    const void* next;
    uint32_t providerType;
};

// Propiedades del sistema (se encadenan en XrSystemProperties.next).
struct XfvSystemSpatialSensingProperties {
    XrStructureType type;
    void* next;
    XrBool32 supportsSpatialSensing;
};

struct XfvSystemSpatialAnchorProperties {
    XrStructureType type;
    void* next;
    XrBool32 supportsSpatialAnchor;
};

struct XfvSenseDataProviderStartInfo {
    XrStructureType type;
    const void* next;
};

struct XfvSenseDataQueryInfo {
    XrStructureType type;
    const void* next;
};

struct XfvSenseDataFilterPlaneOrientation {
    XrStructureType type;
    const void* next;
    uint32_t orientationCount;
    uint32_t* orientations;
};

struct XfvSenseDataFilterUuid {
    XrStructureType type;
    const void* next;
    uint32_t uuidCount;
    const XrUuidEXT* uuids;
};

struct XfvSenseDataQueryCompletion {
    XrStructureType type;
    void* next;
    XrResult futureResult;
    XfvSenseDataSnapshotBD snapshot;
};

struct XfvFutureCompletion {
    XrStructureType type;
    void* next;
    XrResult futureResult;
};

struct XfvQueriedSenseDataGetInfo {
    XrStructureType type;
    const void* next;
};

struct XfvSpatialEntityState {
    XrStructureType type;
    void* next;
    XfvSpatialEntityIdBD entityId;
    XrTime lastUpdateTime;
    XrUuidEXT uuid;
};

struct XfvQueriedSenseData {
    XrStructureType type;
    void* next;
    uint32_t stateCapacityInput;
    uint32_t stateCountOutput;
    XfvSpatialEntityState* states;
};

struct XfvSpatialEntityComponentGetInfo {
    XrStructureType type;
    const void* next;
    XfvSpatialEntityIdBD entityId;
    uint32_t componentType;
};

struct XfvSpatialEntityLocationGetInfo {
    XrStructureType type;
    const void* next;
    XfvSpatialEntityIdBD entityId;
    uint32_t componentType;
    XrSpace baseSpace;
};

struct XfvSpatialEntityComponentDataBaseHeader {
    XrStructureType type;
    void* next;
};

struct XfvSpatialEntityComponentDataLocation {
    XrStructureType type;
    void* next;
    XrSpaceLocation location;
};

struct XfvSpatialEntityComponentDataPolygon {
    XrStructureType type;
    void* next;
    uint32_t vertexCapacityInput;
    uint32_t vertexCountOutput;
    XrVector2f* vertices;
};

struct XfvSpatialEntityComponentDataPlaneOrientation {
    XrStructureType type;
    void* next;
    uint32_t orientation;
};

struct XfvSpatialEntityAnchorCreateInfo {
    XrStructureType type;
    const void* next;
    XfvSenseDataSnapshotBD snapshot;
    XfvSpatialEntityIdBD entityId;
};

struct XfvAnchorSpaceCreateInfo {
    XrStructureType type;
    const void* next;
    XfvAnchorBD anchor;
    XrPosef poseInAnchorSpace;
};

struct XfvSpatialAnchorCreateInfo {
    XrStructureType type;
    const void* next;
    XrSpace space;
    XrPosef pose;
    XrTime time;
};

struct XfvSpatialAnchorCreateCompletion {
    XrStructureType type;
    void* next;
    XrResult futureResult;
    XrUuidEXT uuid;
    XfvAnchorBD anchor;
};

struct XfvSpatialAnchorPersistInfo {
    XrStructureType type;
    const void* next;
    uint32_t location;
    XfvAnchorBD anchor;
};

struct XfvSpatialAnchorUnpersistInfo {
    XrStructureType type;
    const void* next;
    uint32_t location;
    XfvAnchorBD anchor;
};

// --- Punteros a funciones --------------------------------------------------

typedef XrResult(XRAPI_PTR* PFN_CreateSenseDataProviderBD)(XrSession,
                                                            const XfvSenseDataProviderCreateInfo*,
                                                            XfvSenseDataProviderBD*);
typedef XrResult(XRAPI_PTR* PFN_StartSenseDataProviderAsyncBD)(XfvSenseDataProviderBD,
                                                                const XfvSenseDataProviderStartInfo*,
                                                                XrFutureEXT*);
typedef XrResult(XRAPI_PTR* PFN_StartSenseDataProviderCompleteBD)(XrSession, XrFutureEXT,
                                                                  XfvFutureCompletion*);
typedef XrResult(XRAPI_PTR* PFN_GetSenseDataProviderStateBD)(XfvSenseDataProviderBD, uint32_t*);
typedef XrResult(XRAPI_PTR* PFN_QuerySenseDataAsyncBD)(XfvSenseDataProviderBD,
                                                       const XfvSenseDataQueryInfo*, XrFutureEXT*);
typedef XrResult(XRAPI_PTR* PFN_QuerySenseDataCompleteBD)(XfvSenseDataProviderBD, XrFutureEXT,
                                                           XfvSenseDataQueryCompletion*);
typedef XrResult(XRAPI_PTR* PFN_GetQueriedSenseDataBD)(XfvSenseDataSnapshotBD,
                                                        XfvQueriedSenseDataGetInfo*,
                                                        XfvQueriedSenseData*);
typedef XrResult(XRAPI_PTR* PFN_DestroySenseDataSnapshotBD)(XfvSenseDataSnapshotBD);
typedef XrResult(XRAPI_PTR* PFN_StopSenseDataProviderBD)(XfvSenseDataProviderBD);
typedef XrResult(XRAPI_PTR* PFN_DestroySenseDataProviderBD)(XfvSenseDataProviderBD);
typedef XrResult(XRAPI_PTR* PFN_GetSpatialEntityComponentDataBD)(XfvSenseDataSnapshotBD,
                                                                  const XfvSpatialEntityComponentGetInfo*,
                                                                  XfvSpatialEntityComponentDataBaseHeader*);
typedef XrResult(XRAPI_PTR* PFN_GetSpatialEntityUuidBD)(XfvSenseDataSnapshotBD,
                                                        XfvSpatialEntityIdBD, XrUuidEXT*);
typedef XrResult(XRAPI_PTR* PFN_CreateSpatialEntityAnchorBD)(XfvSenseDataProviderBD,
                                                              const XfvSpatialEntityAnchorCreateInfo*,
                                                              XfvAnchorBD*);
typedef XrResult(XRAPI_PTR* PFN_DestroyAnchorBD)(XfvAnchorBD);
typedef XrResult(XRAPI_PTR* PFN_GetAnchorUuidBD)(XfvAnchorBD, XrUuidEXT*);
typedef XrResult(XRAPI_PTR* PFN_CreateAnchorSpaceBD)(XrSession, const XfvAnchorSpaceCreateInfo*,
                                                     XrSpace*);
typedef XrResult(XRAPI_PTR* PFN_CreateSpatialAnchorAsyncBD)(XfvSenseDataProviderBD,
                                                             const XfvSpatialAnchorCreateInfo*,
                                                             XrFutureEXT*);
typedef XrResult(XRAPI_PTR* PFN_CreateSpatialAnchorCompleteBD)(XfvSenseDataProviderBD, XrFutureEXT,
                                                                XfvSpatialAnchorCreateCompletion*);
typedef XrResult(XRAPI_PTR* PFN_PersistSpatialAnchorAsyncBD)(XfvSenseDataProviderBD,
                                                             const XfvSpatialAnchorPersistInfo*,
                                                             XrFutureEXT*);
typedef XrResult(XRAPI_PTR* PFN_PersistSpatialAnchorCompleteBD)(XfvSenseDataProviderBD, XrFutureEXT,
                                                                XfvFutureCompletion*);
typedef XrResult(XRAPI_PTR* PFN_UnpersistSpatialAnchorAsyncBD)(XfvSenseDataProviderBD,
                                                               const XfvSpatialAnchorUnpersistInfo*,
                                                               XrFutureEXT*);
typedef XrResult(XRAPI_PTR* PFN_UnpersistSpatialAnchorCompleteBD)(XfvSenseDataProviderBD,
                                                                  XrFutureEXT,
                                                                  XfvFutureCompletion*);

}  // namespace bd
}  // namespace xfv

// Alias corto para usar la ABI BD desde el estado interno.
namespace bd = xfv::bd;

// ============================================================================
// ESTADO INTERNO DEL MÓDULO
// ============================================================================

namespace {

constexpr char kTag[] = "xfv-ancla";

enum class Estado {
    kSinExtension,   // el runtime no anuncia las extensiones
    kInactivo,       // las anuncia pero no están habilitadas / fallan
    kListo,          // inicializado, esperando a que se pida detectar la mesa
    kEsperandoMesa,  // providers arrancados, buscando la superficie
    kAnclando,       // creando el ancla sobre la superficie detectada
    kAnclado,        // ancla válida y trackeada
    kPersistido,     // ancla válida y además guardada en disco
    kMesaPerdida,    // se perdió el tracking de la mesa
};

struct Proveedor {
    bd::XfvSenseDataProviderBD handle = XR_NULL_HANDLE;
    bool arrancando = false;
    bool arrancado = false;
    XrFutureEXT future = XR_NULL_HANDLE;
};

struct EstadoGlobal {
    bool iniciado = false;
    bool disponible = false;
    Estado estado = Estado::kSinExtension;

    XrInstance instancia = XR_NULL_HANDLE;
    XrSession sesion = XR_NULL_HANDLE;
    XrSystemId sistema = XR_NULL_SYSTEM_ID;
    XrSpace espacioStage = XR_NULL_HANDLE;

    // Extensiones detectadas
    bool extSensing = false;
    bool extAnchor = false;
    bool extPlane = false;
    bool extScene = false;
    bool extPersistencia = false;

    // Punteros a funciones
    bd::PFN_CreateSenseDataProviderBD crearProveedor = nullptr;
    bd::PFN_StartSenseDataProviderAsyncBD startAsync = nullptr;
    bd::PFN_StartSenseDataProviderCompleteBD startComplete = nullptr;
    bd::PFN_GetSenseDataProviderStateBD estadoProveedor = nullptr;
    bd::PFN_QuerySenseDataAsyncBD queryAsync = nullptr;
    bd::PFN_QuerySenseDataCompleteBD queryComplete = nullptr;
    bd::PFN_GetQueriedSenseDataBD getQueried = nullptr;
    bd::PFN_DestroySenseDataSnapshotBD destroySnapshot = nullptr;
    bd::PFN_StopSenseDataProviderBD stopProveedor = nullptr;
    bd::PFN_DestroySenseDataProviderBD destroyProveedor = nullptr;
    bd::PFN_GetSpatialEntityComponentDataBD getComponentData = nullptr;
    bd::PFN_GetSpatialEntityUuidBD getUuid = nullptr;
    bd::PFN_CreateSpatialEntityAnchorBD crearAnchorEntidad = nullptr;
    bd::PFN_DestroyAnchorBD destroyAnchor = nullptr;
    bd::PFN_GetAnchorUuidBD getAnchorUuid = nullptr;
    bd::PFN_CreateAnchorSpaceBD crearAnchorSpace = nullptr;
    bd::PFN_CreateSpatialAnchorAsyncBD crearAnchorAsync = nullptr;
    bd::PFN_CreateSpatialAnchorCompleteBD crearAnchorComplete = nullptr;
    bd::PFN_PersistSpatialAnchorAsyncBD persistirAsync = nullptr;
    bd::PFN_PersistSpatialAnchorCompleteBD persistirComplete = nullptr;
    bd::PFN_UnpersistSpatialAnchorAsyncBD unpersistirAsync = nullptr;
    bd::PFN_UnpersistSpatialAnchorCompleteBD unpersistirComplete = nullptr;

    Proveedor provPlanos;
    Proveedor provAnchors;

    // Consulta de sense data en curso
    bool consultaEnCurso = false;
    XrFutureEXT futureConsulta = XR_NULL_HANDLE;
    bd::XfvSenseDataSnapshotBD snapshot = XR_NULL_HANDLE;
    int framesDesdeConsulta = 0;

    // Candidato actual (superficie de la mesa)
    bool hayCandidato = false;
    bd::XfvSpatialEntityIdBD candidatoId = 0;
    int framesCandidato = 0;

    // Ancla activa
    bool anclaValida = false;
    bool trackingOk = false;
    bd::XfvAnchorBD anchor = XR_NULL_HANDLE;
    XrSpace espacioAnchor = XR_NULL_HANDLE;
    XrPosef poseAncla = {{0.0f, 0.0f, 0.0f, 1.0f}, {0.0f, 0.0f, 0.0f}};
    float matriz[16] = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};
    int framesSinTracking = 0;

    // Persistencia async
    bool persistiendo = false;
    XrFutureEXT futurePersistencia = XR_NULL_HANDLE;
    std::string etiquetaPersistencia;

    // Tabla etiqueta -> uuid (persistencia)
    std::vector<std::pair<std::string, XrUuidEXT>> tablaPersistencia;
    std::string rutaPersistencia;
};

EstadoGlobal g;

// ---------------------------------------------------------------------------
// Utilidades
// ---------------------------------------------------------------------------

void logInfo(const char* fmt, ...) {
    va_list args;
    va_start(args, fmt);
    __android_log_vprint(ANDROID_LOG_INFO, kTag, fmt, args);
    va_end(args);
}

void logWarn(const char* fmt, ...) {
    va_list args;
    va_start(args, fmt);
    __android_log_vprint(ANDROID_LOG_WARN, kTag, fmt, args);
    va_end(args);
}

// Matriz de pose (column-major) a partir de una XrPosef.
void poseAMatriz(const XrPosef& p, float* m) {
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

    m[0] = r00;  m[4] = r10;  m[8]  = r20;  m[12] = 0.0f;
    m[1] = r01;  m[5] = r11;  m[9]  = r21;  m[13] = 0.0f;
    m[2] = r02;  m[6] = r12;  m[10] = r22;  m[14] = 0.0f;
    m[3] = 0.0f; m[7] = 0.0f; m[11] = 0.0f; m[15] = 1.0f;

    m[12] = p.position.x;
    m[13] = p.position.y;
    m[14] = p.position.z;
}

void identidad(float* m) {
    memset(m, 0, 16 * sizeof(float));
    m[0] = m[5] = m[10] = m[15] = 1.0f;
}

std::string uuidACadena(const XrUuidEXT& uuid) {
    static const char* hex = "0123456789abcdef";
    std::string out;
    out.reserve(32);
    for (int i = 0; i < 16; ++i) {
        out.push_back(hex[(uuid.data[i] >> 4) & 0xF]);
        out.push_back(hex[uuid.data[i] & 0xF]);
    }
    return out;
}

bool uuidDesdeCadena(const std::string& s, XrUuidEXT* uuid) {
    if (s.size() != 32) return false;
    auto nibble = [](char c) -> int {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return c - 'a' + 10;
        if (c >= 'A' && c <= 'F') return c - 'A' + 10;
        return -1;
    };
    for (int i = 0; i < 16; ++i) {
        int hi = nibble(s[static_cast<size_t>(i * 2)]);
        int lo = nibble(s[static_cast<size_t>(i * 2 + 1)]);
        if (hi < 0 || lo < 0) return false;
        uuid->data[i] = static_cast<uint8_t>((hi << 4) | lo);
    }
    return true;
}

template <typename T>
T cargar(XrInstance inst, const char* nombre) {
    T fn = nullptr;
    if (xrGetInstanceProcAddr(inst, nombre, reinterpret_cast<PFN_xrVoidFunction*>(&fn)) !=
        XR_SUCCESS) {
        return nullptr;
    }
    return fn;
}

bool tieneExtension(XrInstance instancia, const char* nombre) {
    (void)instancia;
    uint32_t count = 0;
    if (xrEnumerateInstanceExtensionProperties(nullptr, 0, &count, nullptr) != XR_SUCCESS) return false;
    if (count == 0) return false;
    std::vector<XrExtensionProperties> props(count);
    for (auto& p : props) {
        p.type = XR_TYPE_EXTENSION_PROPERTIES;
        p.next = nullptr;
        p.extensionName[0] = '\0';
        p.extensionVersion = 0;
    }
    if (xrEnumerateInstanceExtensionProperties(nullptr, count, &count, props.data()) != XR_SUCCESS)
        return false;
    for (const auto& p : props) {
        if (strncmp(p.extensionName, nombre, XR_MAX_EXTENSION_NAME_SIZE) == 0) return true;
    }
    return false;
}

// ---------------------------------------------------------------------------
// Persistencia en fichero (etiqueta -> uuid)
// ---------------------------------------------------------------------------

void cargarTablaPersistencia() {
    g.tablaPersistencia.clear();
    if (g.rutaPersistencia.empty()) return;
    FILE* f = fopen(g.rutaPersistencia.c_str(), "r");
    if (!f) return;
    char linea[256];
    while (fgets(linea, sizeof(linea), f)) {
        std::string s(linea);
        while (!s.empty() && (s.back() == '\n' || s.back() == '\r')) s.pop_back();
        const size_t sep = s.find(' ');
        if (sep == std::string::npos) continue;
        XrUuidEXT uuid{};
        if (!uuidDesdeCadena(s.substr(sep + 1), &uuid)) continue;
        g.tablaPersistencia.emplace_back(s.substr(0, sep), uuid);
    }
    fclose(f);
    logInfo("Cargadas %zu anclas persistidas de '%s'", g.tablaPersistencia.size(),
            g.rutaPersistencia.c_str());
}

void guardarTablaPersistencia() {
    if (g.rutaPersistencia.empty()) return;
    FILE* f = fopen(g.rutaPersistencia.c_str(), "w");
    if (!f) {
        logWarn("No se pudo escribir '%s'", g.rutaPersistencia.c_str());
        return;
    }
    for (const auto& par : g.tablaPersistencia) {
        fprintf(f, "%s %s\n", par.first.c_str(), uuidACadena(par.second).c_str());
    }
    fclose(f);
}

void anotarPersistencia(const std::string& etiqueta, const XrUuidEXT& uuid) {
    for (auto& par : g.tablaPersistencia) {
        if (par.first == etiqueta) {
            par.second = uuid;
            guardarTablaPersistencia();
            return;
        }
    }
    g.tablaPersistencia.emplace_back(etiqueta, uuid);
    guardarTablaPersistencia();
}

bool buscarPersistencia(const std::string& etiqueta, XrUuidEXT* uuid) {
    for (const auto& par : g.tablaPersistencia) {
        if (par.first == etiqueta) {
            if (uuid) *uuid = par.second;
            return true;
        }
    }
    return false;
}

// ---------------------------------------------------------------------------
// Proveedores de sense data
// ---------------------------------------------------------------------------

bool crearYArrancarProveedor(Proveedor* salida, uint32_t tipoProveedor) {
    bd::XfvSenseDataProviderCreateInfo createInfo{};
    createInfo.type = bd::kTypeSenseDataProviderCreateInfo;
    createInfo.next = nullptr;
    createInfo.providerType = tipoProveedor;

    bd::XfvSenseDataProviderBD provider = XR_NULL_HANDLE;
    XrResult res = g.crearProveedor(g.sesion, &createInfo, &provider);
    if (res != XR_SUCCESS) {
        logWarn("xrCreateSenseDataProviderBD(tipo=%u) falló: %d", tipoProveedor, (int)res);
        return false;
    }

    bd::XfvSenseDataProviderStartInfo startInfo{};
    startInfo.type = bd::kTypeSenseDataProviderStartInfo;
    startInfo.next = nullptr;

    XrFutureEXT future = XR_NULL_HANDLE;
    res = g.startAsync(provider, &startInfo, &future);
    if (res != XR_SUCCESS) {
        logWarn("xrStartSenseDataProviderAsyncBD falló: %d", (int)res);
        g.destroyProveedor(provider);
        return false;
    }

    salida->handle = provider;
    salida->arrancando = true;
    salida->arrancado = false;
    salida->future = future;
    return true;
}

// Comprueba si un provider ha terminado de arrancar.
bool providerArranco(Proveedor& prov) {
    if (!prov.arrancando || prov.arrancado) return prov.arrancado;
    bd::XfvFutureCompletion completion{};
    completion.type = bd::kTypeFutureCompletionExt;
    completion.next = nullptr;
    completion.futureResult = XR_SUCCESS;
    XrResult res = g.startComplete(g.sesion, prov.future, &completion);
    if (res == bd::kErrorFuturePendingExt || res == XR_SUCCESS) {
        // XR_SUCCESS aquí significa "future resuelto": el completion trae el
        // resultado real.
        if (res == XR_SUCCESS && completion.futureResult == XR_SUCCESS) {
            prov.arrancado = true;
            prov.arrancando = false;
            logInfo("Proveedor de sense data arrancado");
            return true;
        }
        if (res == XR_SUCCESS && completion.futureResult != XR_SUCCESS) {
            logWarn("El proveedor de sense data se detuvo: %d", (int)completion.futureResult);
            prov.arrancando = false;
            prov.arrancado = false;
        }
        return prov.arrancado;
    }
    logWarn("xrStartSenseDataProviderCompleteBD falló: %d", (int)res);
    prov.arrancando = false;
    return false;
}

void destruirProveedores() {
    if (g.snapshot && g.destroySnapshot) {
        g.destroySnapshot(g.snapshot);
        g.snapshot = XR_NULL_HANDLE;
    }
    Proveedor* lista[2] = {&g.provPlanos, &g.provAnchors};
    for (Proveedor* p : lista) {
        if (!p->handle) continue;
        if (p->arrancado && g.stopProveedor) g.stopProveedor(p->handle);
        if (g.destroyProveedor) g.destroyProveedor(p->handle);
        p->handle = XR_NULL_HANDLE;
        p->arrancado = false;
        p->arrancando = false;
    }
}

void destruirAncla() {
    if (g.espacioAnchor) {
        xrDestroySpace(g.espacioAnchor);
        g.espacioAnchor = XR_NULL_HANDLE;
    }
    if (g.anchor) {
        if (g.destroyAnchor) g.destroyAnchor(g.anchor);
        g.anchor = XR_NULL_HANDLE;
    }
    g.anclaValida = false;
    g.trackingOk = false;
}

// ---------------------------------------------------------------------------
// Descubrimiento de la mesa
// ---------------------------------------------------------------------------

struct Candidato {
    bd::XfvSpatialEntityIdBD id = 0;
    XrPosef pose = {{0.0f, 0.0f, 0.0f, 1.0f}, {0.0f, 0.0f, 0.0f}};
    float semiExtent = 0.0f;  // metros (radio aproximado del plano)
    bool valido = false;
};

// Interroga a un sense provider y devuelve las superficies candidatas.
std::vector<Candidato> buscarSuperficies(bd::XfvSenseDataSnapshotBD snapshot) {
    std::vector<Candidato> salida;
    if (!snapshot || !g.getQueried) return salida;

    bd::XfvQueriedSenseDataGetInfo getInfo{};
    getInfo.type = bd::kTypeQueriedSenseDataGetInfo;
    getInfo.next = nullptr;

    uint32_t capacidad = 0;
    // Primera llamada con capacidad 0: el runtime escribe el número de
    // estados en stateCountOutput.
    struct CountProbe {
        XrStructureType type;
        void* next;
        uint32_t stateCapacityInput;
        uint32_t stateCountOutput;
        bd::XfvSpatialEntityState* states;
    } probe{};
    probe.type = bd::kTypeQueriedSenseData;
    probe.next = nullptr;
    probe.stateCapacityInput = 0;
    probe.stateCountOutput = 0;
    probe.states = nullptr;
    XrResult res = g.getQueried(snapshot, &getInfo,
                                 reinterpret_cast<bd::XfvQueriedSenseData*>(&probe));
    if (res != XR_SUCCESS) return salida;
    capacidad = probe.stateCountOutput;
    if (capacidad == 0) return salida;
    if (capacidad > 128) capacidad = 128;  // tope de seguridad

    std::vector<bd::XfvSpatialEntityState> estados(capacidad);
    for (auto& e : estados) {
        e.type = bd::kTypeSpatialEntityState;
        e.next = nullptr;
        e.entityId = 0;
        e.lastUpdateTime = 0;
        memset(e.uuid.data, 0, sizeof(e.uuid.data));
    }
    bd::XfvQueriedSenseData queried{};
    queried.type = bd::kTypeQueriedSenseData;
    queried.next = nullptr;
    queried.stateCapacityInput = capacidad;
    queried.stateCountOutput = 0;
    queried.states = estados.data();
    res = g.getQueried(snapshot, &getInfo, &queried);
    if (res != XR_SUCCESS) return salida;

    const uint32_t n = queried.stateCountOutput < capacidad ? queried.stateCountOutput : capacidad;
    for (uint32_t i = 0; i < n; ++i) {
        const bd::XfvSpatialEntityIdBD id = estados[i].entityId;

        // 1) Orientación del plano: solo nos valen los horizontales hacia
        //    arriba (mesas, suelos).
        bd::XfvSpatialEntityComponentGetInfo getComp{};
        getComp.type = bd::kTypeSpatialEntityComponentGetInfo;
        getComp.next = nullptr;
        getComp.entityId = id;
        getComp.componentType = bd::kComponentTypePlaneOrientation;

        bd::XfvSpatialEntityComponentDataPlaneOrientation orient{};
        orient.type = bd::kTypeSpatialEntityComponentDataPlaneOrientation;
        orient.next = nullptr;
        orient.orientation = bd::kPlaneOrientationArbitrary;

        bool esMesa = false;
        if (g.getComponentData) {
            XrResult r = g.getComponentData(
                snapshot, &getComp,
                reinterpret_cast<bd::XfvSpatialEntityComponentDataBaseHeader*>(&orient));
            if (r == XR_SUCCESS && orient.orientation == bd::kPlaneOrientationHorizontalUpward) {
                esMesa = true;
            }
        }

        // 2) Posición: pedimos la.location en nuestro espacio STAGE.
        bd::XfvSpatialEntityLocationGetInfo getLoc{};
        getLoc.type = bd::kTypeSpatialEntityLocationGetInfo;
        getLoc.next = nullptr;
        getLoc.entityId = id;
        getLoc.componentType = bd::kComponentTypeLocation;
        getLoc.baseSpace = g.espacioStage;

        bd::XfvSpatialEntityComponentDataLocation loc{};
        loc.type = bd::kTypeSpatialEntityComponentDataLocation;
        loc.next = nullptr;
        loc.location.type = XR_TYPE_SPACE_LOCATION;
        loc.location.next = nullptr;
        loc.location.pose = {{0.0f, 0.0f, 0.0f, 1.0f}, {0.0f, 0.0f, 0.0f}};
        loc.location.locationFlags = 0;

        if (!g.getComponentData) continue;
        XrResult r = g.getComponentData(
            snapshot, reinterpret_cast<const bd::XfvSpatialEntityComponentGetInfo*>(&getLoc),
            reinterpret_cast<bd::XfvSpatialEntityComponentDataBaseHeader*>(&loc));
        if (r != XR_SUCCESS) continue;
        if ((loc.location.locationFlags & XR_SPACE_LOCATION_POSITION_VALID_BIT) == 0) continue;
        if ((loc.location.locationFlags & XR_SPACE_LOCATION_POSITION_TRACKED_BIT) == 0) continue;

        // 3) Tamaño aproximado (polígono en metros).
        float semi = 0.0f;
        bd::XfvSpatialEntityComponentGetInfo getPoly{};
        getPoly.type = bd::kTypeSpatialEntityComponentGetInfo;
        getPoly.next = nullptr;
        getPoly.entityId = id;
        getPoly.componentType = bd::kComponentTypePolygon;

        bd::XfvSpatialEntityComponentDataPolygon poly{};
        poly.type = bd::kTypeSpatialEntityComponentDataPolygon;
        poly.next = nullptr;
        poly.vertexCapacityInput = 0;
        poly.vertexCountOutput = 0;
        poly.vertices = nullptr;
        XrResult rp = g.getComponentData(
            snapshot, &getPoly, reinterpret_cast<bd::XfvSpatialEntityComponentDataBaseHeader*>(&poly));
        if (rp == XR_SUCCESS && poly.vertexCountOutput > 0 && poly.vertexCountOutput < 256) {
            std::vector<XrVector2f> verts(poly.vertexCountOutput);
            poly.vertexCapacityInput = poly.vertexCountOutput;
            poly.vertices = verts.data();
            rp = g.getComponentData(
                snapshot, &getPoly, reinterpret_cast<bd::XfvSpatialEntityComponentDataBaseHeader*>(&poly));
            if (rp == XR_SUCCESS) {
                for (const auto& v : verts) {
                    semi = fmaxf(semi, fmaxf(fabsf(v.x), fabsf(v.y)));
                }
            }
        }

        // Si el runtime no da polígono, usamos la caja 2D.
        if (semi <= 0.0f) {
            bd::XfvSpatialEntityComponentGetInfo getBox{};
            getBox.type = bd::kTypeSpatialEntityComponentGetInfo;
            getBox.next = nullptr;
            getBox.entityId = id;
            getBox.componentType = bd::kComponentTypeBoundingBox2D;
            struct BoxData {
                XrStructureType type;
                void* next;
                XrRect2Df box;
            } box{};
            box.type = bd::kTypeSpatialEntityComponentDataBoundingBox2D;
            box.next = nullptr;
            box.box = {{0.0f, 0.0f}, {0.0f, 0.0}};
            XrResult rb = g.getComponentData(
                snapshot, &getBox,
                reinterpret_cast<bd::XfvSpatialEntityComponentDataBaseHeader*>(&box));
            if (rb == XR_SUCCESS) {
                semi = 0.5f * fmaxf(fabsf(box.box.extent.width), fabsf(box.box.extent.height));
            }
        }

        Candidato c;
        c.id = id;
        c.pose = loc.location.pose;
        c.semiExtent = semi;
        // Solo aceptamos superficies con aspecto de mesa: entre 15 cm y 2 m de
        // radio y a menos de 2 m del suelo del STAGE.
        c.valido = esMesa && semi > 0.15f && semi < 2.0f && loc.location.pose.position.y < 2.0f &&
                   loc.location.pose.position.y > 0.1f;
        if (c.valido) salida.push_back(c);
    }
    return salida;
}

// Crea el ancla sobre el sense provider indicado.
bool crearAnclaSobreEntidad(bd::XfvSenseDataProviderBD provider,
                             bd::XfvSenseDataSnapshotBD snapshot,
                             bd::XfvSpatialEntityIdBD id) {
    if (!provider || !snapshot || !g.crearAnchorEntidad || !g.crearAnchorSpace) return false;

    bd::XfvSpatialEntityAnchorCreateInfo info{};
    info.type = bd::kTypeSpatialEntityAnchorCreateInfo;
    info.next = nullptr;
    info.snapshot = snapshot;
    info.entityId = id;

    bd::XfvAnchorBD anchor = XR_NULL_HANDLE;
    XrResult res = g.crearAnchorEntidad(provider, &info, &anchor);
    if (res != XR_SUCCESS) {
        logWarn("xrCreateSpatialEntityAnchorBD falló: %d", (int)res);
        return false;
    }

    bd::XfvAnchorSpaceCreateInfo spaceInfo{};
    spaceInfo.type = bd::kTypeAnchorSpaceCreateInfo;
    spaceInfo.next = nullptr;
    spaceInfo.anchor = anchor;
    spaceInfo.poseInAnchorSpace = {{0.0f, 0.0f, 0.0f, 1.0f}, {0.0f, 0.0f, 0.0f}};

    XrSpace space = XR_NULL_HANDLE;
    res = g.crearAnchorSpace(g.sesion, &spaceInfo, &space);
    if (res != XR_SUCCESS) {
        logWarn("xrCreateAnchorSpaceBD falló: %d", (int)res);
        if (g.destroyAnchor) g.destroyAnchor(anchor);
        return false;
    }

    destruirAncla();
    g.anchor = anchor;
    g.espacioAnchor = space;
    g.anclaValida = true;
    g.framesSinTracking = 0;
    logInfo("Ancla creada sobre la superficie %llu", (unsigned long long)id);
    return true;
}

}  // namespace

// ============================================================================
// API PÚBLICA
// ============================================================================

namespace xfv {

bool ancla_espacial_iniciar(XrInstance instancia, XrSession sesion, XrSystemId sistema) {
    g.instancia = instancia;
    g.sesion = sesion;
    g.sistema = sistema;
    g.estado = Estado::kSinExtension;

    if (instancia == XR_NULL_HANDLE || sesion == XR_NULL_HANDLE) {
        logWarn("Iniciar con handles nulos; el anclaje queda inactivo");
        g.estado = Estado::kInactivo;
        return false;
    }

    // --- 1) ¿Qué extensiones anuncia el runtime? -------------------------
    g.extSensing = tieneExtension(instancia, "XR_BD_spatial_sensing");
    g.extAnchor = tieneExtension(instancia, "XR_BD_spatial_anchor");
    g.extPlane = tieneExtension(instancia, "XR_BD_spatial_plane");
    g.extScene = tieneExtension(instancia, "XR_BD_spatial_scene");
    // Algunas builds anuncian la persistencia como extensión aparte; la API
    // real está dentro de XR_BD_spatial_anchor, así que se trata como alias.
    g.extPersistencia = g.extAnchor || tieneExtension(instancia, "XR_BD_spatial_anchor_persistence");

    logInfo("Extensiones: sensing=%d anchor=%d plane=%d scene=%d persist=%d", g.extSensing,
            g.extAnchor, g.extPlane, g.extScene, g.extPersistencia);

    if (!g.extSensing || !g.extAnchor) {
        logWarn("Sin XR_BD_spatial_sensing/anchor: se sigue con STAGE");
        g.estado = Estado::kSinExtension;
        g.disponible = false;
        g.iniciado = true;
        return false;
    }
    if (!g.extPlane && !g.extScene) {
        logWarn("Sin XR_BD_spatial_plane ni XR_BD_spatial_scene: sin detección de mesa");
        g.estado = Estado::kSinExtension;
        g.disponible = false;
        g.iniciado = true;
        return false;
    }

    // --- 2) Cargar los punteros a función ---------------------------------
    g.crearProveedor = cargar<bd::PFN_CreateSenseDataProviderBD>(instancia, "xrCreateSenseDataProviderBD");
    g.startAsync = cargar<bd::PFN_StartSenseDataProviderAsyncBD>(instancia, "xrStartSenseDataProviderAsyncBD");
    g.startComplete = cargar<bd::PFN_StartSenseDataProviderCompleteBD>(instancia, "xrStartSenseDataProviderCompleteBD");
    g.estadoProveedor = cargar<bd::PFN_GetSenseDataProviderStateBD>(instancia, "xrGetSenseDataProviderStateBD");
    g.queryAsync = cargar<bd::PFN_QuerySenseDataAsyncBD>(instancia, "xrQuerySenseDataAsyncBD");
    g.queryComplete = cargar<bd::PFN_QuerySenseDataCompleteBD>(instancia, "xrQuerySenseDataCompleteBD");
    g.getQueried = cargar<bd::PFN_GetQueriedSenseDataBD>(instancia, "xrGetQueriedSenseDataBD");
    g.destroySnapshot = cargar<bd::PFN_DestroySenseDataSnapshotBD>(instancia, "xrDestroySenseDataSnapshotBD");
    g.stopProveedor = cargar<bd::PFN_StopSenseDataProviderBD>(instancia, "xrStopSenseDataProviderBD");
    g.destroyProveedor = cargar<bd::PFN_DestroySenseDataProviderBD>(instancia, "xrDestroySenseDataProviderBD");
    g.getComponentData = cargar<bd::PFN_GetSpatialEntityComponentDataBD>(instancia, "xrGetSpatialEntityComponentDataBD");
    g.getUuid = cargar<bd::PFN_GetSpatialEntityUuidBD>(instancia, "xrGetSpatialEntityUuidBD");
    g.crearAnchorEntidad = cargar<bd::PFN_CreateSpatialEntityAnchorBD>(instancia, "xrCreateSpatialEntityAnchorBD");
    g.destroyAnchor = cargar<bd::PFN_DestroyAnchorBD>(instancia, "xrDestroyAnchorBD");
    g.getAnchorUuid = cargar<bd::PFN_GetAnchorUuidBD>(instancia, "xrGetAnchorUuidBD");
    g.crearAnchorSpace = cargar<bd::PFN_CreateAnchorSpaceBD>(instancia, "xrCreateAnchorSpaceBD");
    g.crearAnchorAsync = cargar<bd::PFN_CreateSpatialAnchorAsyncBD>(instancia, "xrCreateSpatialAnchorAsyncBD");
    g.crearAnchorComplete = cargar<bd::PFN_CreateSpatialAnchorCompleteBD>(instancia, "xrCreateSpatialAnchorCompleteBD");
    g.persistirAsync = cargar<bd::PFN_PersistSpatialAnchorAsyncBD>(instancia, "xrPersistSpatialAnchorAsyncBD");
    g.persistirComplete = cargar<bd::PFN_PersistSpatialAnchorCompleteBD>(instancia, "xrPersistSpatialAnchorCompleteBD");
    g.unpersistirAsync = cargar<bd::PFN_UnpersistSpatialAnchorAsyncBD>(instancia, "xrUnpersistSpatialAnchorAsyncBD");
    g.unpersistirComplete = cargar<bd::PFN_UnpersistSpatialAnchorCompleteBD>(instancia, "xrUnpersistSpatialAnchorCompleteBD");

    const bool hayBasicos = g.crearProveedor && g.startAsync && g.startComplete && g.queryAsync &&
                            g.queryComplete && g.getQueried && g.getComponentData &&
                            g.crearAnchorEntidad && g.crearAnchorSpace;
    if (!hayBasicos) {
        logWarn("Faltan funciones de XR_BD_spatial_* en el runtime; anclaje inactivo");
        g.estado = Estado::kInactivo;
        g.disponible = false;
        g.iniciado = true;
        return false;
    }

    // --- 3) ¿El sistema soporta sensing/anchors? --------------------------
    bd::XfvSystemSpatialSensingProperties sensingProps = {};
    sensingProps.type = bd::kTypeSystemSpatialSensingProperties;
    sensingProps.next = nullptr;
    sensingProps.supportsSpatialSensing = XR_FALSE;

    bd::XfvSystemSpatialAnchorProperties anchorProps = {};
    anchorProps.type = bd::kTypeSystemSpatialAnchorProperties;
    anchorProps.next = nullptr;
    anchorProps.supportsSpatialAnchor = XR_FALSE;

    XrSystemProperties props = {};
    props.type = XR_TYPE_SYSTEM_PROPERTIES;
    props.next = &sensingProps;
    sensingProps.next = &anchorProps;
    props.systemId = sistema;
    props.vendorId = 0;

    XrResult res = xrGetSystemProperties(instancia, sistema, &props);
    if (res == XR_SUCCESS) {
        if (sensingProps.supportsSpatialSensing != XR_TRUE) {
            logWarn("El sistema no soporta spatial sensing");
            g.estado = Estado::kInactivo;
            g.disponible = false;
            g.iniciado = true;
            return false;
        }
        if (anchorProps.supportsSpatialAnchor != XR_TRUE) {
            logWarn("El sistema no soporta spatial anchors");
            g.estado = Estado::kInactivo;
            g.disponible = false;
            g.iniciado = true;
            return false;
        }
    } else {
        // Un runtime antiguo puede no conocer estas estructuras: no es fatal.
        logWarn("xrGetSystemProperties con props BD devolvió %d (se continúa)", (int)res);
    }

    // --- 4) Espacio STAGE propio (para expresar la pose del ancla) --------
    XrReferenceSpaceCreateInfo refInfo = {};
    refInfo.type = XR_TYPE_REFERENCE_SPACE_CREATE_INFO;
    refInfo.next = nullptr;
    refInfo.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_STAGE;
    refInfo.poseInReferenceSpace = {{0.0f, 0.0f, 0.0f, 1.0f}, {0.0f, 0.0f, 0.0f}};
    res = xrCreateReferenceSpace(sesion, &refInfo, &g.espacioStage);
    if (res != XR_SUCCESS) {
        logWarn("No se pudo crear el espacio STAGE propio (%d); se usa LOCAL", (int)res);
        refInfo.referenceSpaceType = XR_REFERENCE_SPACE_TYPE_LOCAL;
        res = xrCreateReferenceSpace(sesion, &refInfo, &g.espacioStage);
        if (res != XR_SUCCESS) {
            logWarn("Tampoco LOCAL (%d); el anclaje queda inactivo", (int)res);
            g.espacioStage = XR_NULL_HANDLE;
            g.estado = Estado::kInactivo;
            g.disponible = false;
            g.iniciado = true;
            return false;
        }
    }

    g.estado = Estado::kListo;
    g.disponible = true;
    g.iniciado = true;
    cargarTablaPersistencia();
    logInfo("Anclaje espacial activo (STAGE propio=%p)", (void*)g.espacioStage);
    return true;
}

bool ancla_espacial_disponible() { return g.disponible && g.iniciado; }

bool ancla_espacial_preparar_mesa() {
    if (!ancla_espacial_disponible()) return false;

    if (g.provPlanos.handle || g.provAnchors.handle) {
        // Ya estaba preparado.
        return true;
    }

    // Proveedor de superficies (planos; si no, la escena completa).
    bool ok = false;
    if (g.extPlane) {
        ok = crearYArrancarProveedor(&g.provPlanos, bd::kProviderTypePlane);
    }
    if (!ok && g.extScene) {
        ok = crearYArrancarProveedor(&g.provPlanos, bd::kProviderTypeScene);
    }
    if (!ok) {
        logWarn("No se pudo arrancar el proveedor de superficies");
        g.estado = Estado::kInactivo;
        return false;
    }

    // Proveedor de anclas (necesario para crear el ancla sobre la entidad).
    if (!crearYArrancarProveedor(&g.provAnchors, bd::kProviderTypeAnchor)) {
        logWarn("No se pudo arrancar el proveedor de anclas");
        destruirProveedores();
        g.estado = Estado::kInactivo;
        return false;
    }

    g.hayCandidato = false;
    g.framesCandidato = 0;
    g.estado = Estado::kEsperandoMesa;
    logInfo("Arrancada la captura de superficies para buscar la mesa");
    return true;
}

bool ancla_espacial_confirmar_mesa() {
    if (!ancla_espacial_disponible()) return false;
    if (!g.hayCandidato || !g.snapshot) {
        logWarn("Confirmar sin superficie detectada todavía");
        return false;
    }
    g.estado = Estado::kAnclando;
    if (!crearAnclaSobreEntidad(g.provAnchors.handle, g.snapshot, g.candidatoId)) {
        g.estado = Estado::kEsperandoMesa;
        return false;
    }
    g.estado = Estado::kAnclado;
    return true;
}

bool ancla_espacial_actualizar(XrFrameState frame) {
    if (!ancla_espacial_disponible()) return false;

    // --- 1) Arranque de los proveedores -----------------------------------
    if (!g.provPlanos.arrancado) providerArranco(g.provPlanos);
    if (!g.provAnchors.arrancado) providerArranco(g.provAnchors);

    const bool proveedoresListos =
        g.provPlanos.arrancado && g.provAnchors.arrancado;

    // --- 2) Consulta de superficies ---------------------------------------
    if (proveedoresListos && !g.anclaValida) {
        if (g.consultaEnCurso) {
            bd::XfvSenseDataQueryCompletion completion{};
            completion.type = bd::kTypeSenseDataQueryCompletion;
            completion.next = nullptr;
            completion.futureResult = XR_SUCCESS;
            completion.snapshot = XR_NULL_HANDLE;

            XrResult res = g.queryComplete(g.provPlanos.handle, g.futureConsulta, &completion);
            if (res == XR_SUCCESS && completion.futureResult == XR_SUCCESS &&
                completion.snapshot != XR_NULL_HANDLE) {
                g.consultaEnCurso = false;
                if (g.snapshot && g.destroySnapshot) g.destroySnapshot(g.snapshot);
                g.snapshot = completion.snapshot;

                std::vector<Candidato> cands = buscarSuperficies(g.snapshot);
                if (!cands.empty()) {
                    // Elegimos la superficie más grande (más probable mesa).
                    Candidato mejor = cands[0];
                    for (const auto& c : cands) {
                        if (c.semiExtent > mejor.semiExtent) mejor = c;
                    }
                    if (!g.hayCandidato || g.candidatoId != mejor.id) {
                        g.candidatoId = mejor.id;
                        g.framesCandidato = 0;
                    }
                    g.hayCandidato = true;
                    g.framesCandidato++;
                    logInfo("Superficie candidata %llu (radio %.2f m) frame %d",
                            (unsigned long long)mejor.id, mejor.semiExtent, g.framesCandidato);
                    // Auto-anclaje tras 3 detecciones estables seguidas.
                    if (g.framesCandidato >= 3) {
                        g.estado = Estado::kAnclando;
                        if (crearAnclaSobreEntidad(g.provAnchors.handle, g.snapshot,
                                                   mejor.id)) {
                            g.estado = Estado::kAnclado;
                            g.hayCandidato = false;
                        } else {
                            g.estado = Estado::kEsperandoMesa;
                        }
                    }
                } else {
                    g.hayCandidato = false;
                    g.framesCandidato = 0;
                }
            } else if (res != bd::kErrorFuturePendingExt) {
                g.consultaEnCurso = false;
                logWarn("xrQuerySenseDataCompleteBD devolvió %d", (int)res);
            }
        } else {
            g.framesDesdeConsulta++;
            if (g.framesDesdeConsulta >= 8) {
                g.framesDesdeConsulta = 0;

                // Filtro: solo superficies horizontales hacia arriba (mesas).
                uint32_t orientaciones[1] = {bd::kPlaneOrientationHorizontalUpward};
                bd::XfvSenseDataFilterPlaneOrientation filtroOrient{};
                filtroOrient.type = bd::kTypeSenseDataFilterPlaneOrientation;
                filtroOrient.next = nullptr;
                filtroOrient.orientationCount = 1;
                filtroOrient.orientations = orientaciones;

                bd::XfvSenseDataQueryInfo queryInfo{};
                queryInfo.type = bd::kTypeSenseDataQueryInfo;
                queryInfo.next = (g.extPlane && g.provPlanos.handle) ? &filtroOrient : nullptr;

                XrFutureEXT future = XR_NULL_HANDLE;
                XrResult res = g.queryAsync(g.provPlanos.handle, &queryInfo, &future);
                if (res == XR_SUCCESS) {
                    g.futureConsulta = future;
                    g.consultaEnCurso = true;
                } else {
                    logWarn("xrQuerySenseDataAsyncBD devolvió %d", (int)res);
                    if (res == XR_ERROR_FEATURE_UNSUPPORTED) {
                        g.estado = Estado::kInactivo;
                        g.disponible = false;
                    }
                }
            }
        }
    }

    // --- 3) Persistencia asíncrona ----------------------------------------
    if (g.persistiendo && g.futurePersistencia != XR_NULL_HANDLE && g.persistirComplete) {
        bd::XfvFutureCompletion completion{};
        completion.type = bd::kTypeFutureCompletionExt;
        completion.next = nullptr;
        completion.futureResult = XR_SUCCESS;
        XrResult res = g.persistirComplete(g.provAnchors.handle, g.futurePersistencia, &completion);
        if (res == XR_SUCCESS) {
            g.persistiendo = false;
            g.futurePersistencia = XR_NULL_HANDLE;
            if (completion.futureResult == XR_SUCCESS) {
                XrUuidEXT uuid{};
                memset(uuid.data, 0, sizeof(uuid.data));
                if (g.anchor && g.getAnchorUuid &&
                    g.getAnchorUuid(g.anchor, &uuid) == XR_SUCCESS) {
                    anotarPersistencia(g.etiquetaPersistencia, uuid);
                    g.estado = Estado::kPersistido;
                    logInfo("Ancla '%s' persistida (%s)", g.etiquetaPersistencia.c_str(),
                            uuidACadena(uuid).c_str());
                } else {
                    g.estado = Estado::kAnclado;
                }
            } else {
                logWarn("La persistencia falló: %d", (int)completion.futureResult);
                g.estado = Estado::kAnclado;
            }
        } else if (res != bd::kErrorFuturePendingExt) {
            logWarn("xrPersistSpatialAnchorCompleteBD devolvió %d", (int)res);
            g.persistiendo = false;
            g.futurePersistencia = XR_NULL_HANDLE;
            g.estado = Estado::kAnclado;
        }
    }

    // --- 4) Pose del ancla -------------------------------------------------
    if (g.anclaValida && g.espacioAnchor) {
        XrSpaceLocation loc = {};
        loc.type = XR_TYPE_SPACE_LOCATION;
        loc.next = nullptr;
        loc.pose = {{0.0f, 0.0f, 0.0f, 1.0f}, {0.0f, 0.0f, 0.0f}};
        loc.locationFlags = 0;

        XrResult res = xrLocateSpace(g.espacioAnchor, g.espacioStage, frame.predictedDisplayTime,
                                     &loc);
        const bool ok = (res == XR_SUCCESS) &&
                        (loc.locationFlags & XR_SPACE_LOCATION_POSITION_VALID_BIT) &&
                        (loc.locationFlags & XR_SPACE_LOCATION_ORIENTATION_VALID_BIT) &&
                        (loc.locationFlags & XR_SPACE_LOCATION_POSITION_TRACKED_BIT);
        if (ok) {
            g.poseAncla = loc.pose;
            poseAMatriz(g.poseAncla, g.matriz);
            g.trackingOk = true;
            g.framesSinTracking = 0;
            if (g.estado == Estado::kMesaPerdida) g.estado = Estado::kAnclado;
        } else {
            g.trackingOk = false;
            g.framesSinTracking++;
            if (g.framesSinTracking > 30) {
                logWarn("Se perdió el tracking de la mesa");
                g.estado = Estado::kMesaPerdida;
                destruirAncla();
                identidad(g.matriz);
            }
        }
    }

    return g.anclaValida && g.trackingOk;
}

void ancla_espacial_matriz(float* out16) {
    if (!out16) return;
    if (!ancla_espacial_disponible() || !g.anclaValida || !g.trackingOk) {
        identidad(out16);
        return;
    }
    memcpy(out16, g.matriz, 16 * sizeof(float));
}

bool ancla_espacial_persistir(const char* etiqueta) {
    if (!etiqueta || !*etiqueta) return false;
    if (!ancla_espacial_disponible()) return false;
    if (!g.anclaValida || !g.anchor) {
        logWarn("No hay ancla que persistir");
        return false;
    }
    if (!g.persistirAsync || !g.persistirComplete) {
        logWarn("El runtime no expone persistencia de anchors");
        return false;
    }
    if (g.persistiendo) return false;

    bd::XfvSpatialAnchorPersistInfo info{};
    info.type = bd::kTypeSpatialAnchorPersistInfo;
    info.next = nullptr;
    info.location = bd::kPersistenceLocationLocal;
    info.anchor = g.anchor;

    XrFutureEXT future = XR_NULL_HANDLE;
    XrResult res = g.persistirAsync(g.provAnchors.handle, &info, &future);
    if (res != XR_SUCCESS) {
        logWarn("xrPersistSpatialAnchorAsyncBD devolvió %d", (int)res);
        return false;
    }
    g.futurePersistencia = future;
    g.etiquetaPersistencia = etiqueta;
    g.persistiendo = true;
    return true;
}

bool ancla_espacial_restaurar(const char* etiqueta) {
    if (!etiqueta || !*etiqueta) return false;
    if (!ancla_espacial_disponible()) return false;

    XrUuidEXT uuid{};
    if (!buscarPersistencia(etiqueta, &uuid)) {
        logWarn("No hay ancla persistida con la etiqueta '%s'", etiqueta);
        return false;
    }

    if (!g.provAnchors.arrancado) {
        if (!ancla_espacial_preparar_mesa()) return false;
    }

    // Consulta al proveedor de anchors filtrando por UUID.
    bd::XfvSenseDataFilterUuid filtro{};
    filtro.type = bd::kTypeSenseDataFilterUuid;
    filtro.next = nullptr;
    filtro.uuidCount = 1;
    filtro.uuids = &uuid;

    bd::XfvSenseDataQueryInfo queryInfo{};
    queryInfo.type = bd::kTypeSenseDataQueryInfo;
    queryInfo.next = &filtro;

    XrFutureEXT future = XR_NULL_HANDLE;
    XrResult res = g.queryAsync(g.provAnchors.handle, &queryInfo, &future);
    if (res != XR_SUCCESS) {
        logWarn("xrQuerySenseDataAsyncBD (restore) devolvió %d", (int)res);
        return false;
    }

    // La consulta es asíncrona: esperamos al próximo frame para resolverla.
    // Para eso guardamos el filtro; la resolución real se hace aquí de forma
    // síncrona con un spin corto sobre el future.
    bd::XfvSenseDataQueryCompletion completion{};
    completion.type = bd::kTypeSenseDataQueryCompletion;
    completion.next = nullptr;
    completion.futureResult = XR_SUCCESS;
    completion.snapshot = XR_NULL_HANDLE;

    int intentos = 0;
    XrResult r = bd::kErrorFuturePendingExt;
    while (r == bd::kErrorFuturePendingExt && intentos < 60) {
        r = g.queryComplete(g.provAnchors.handle, future, &completion);
        if (r == bd::kErrorFuturePendingExt) {
            ++intentos;
            // Espera corta: el runtime resuelve el future en el bucle de la
            // sesión; aquí damos un respiro mínimo.
            usleep(10000);
        }
    }
    if (r != XR_SUCCESS || completion.futureResult != XR_SUCCESS ||
        completion.snapshot == XR_NULL_HANDLE) {
        logWarn("No se pudo resolver el anchor persistido '%s' (%d)", etiqueta, (int)r);
        return false;
    }

    // Buscamos la entidad con ese UUID y creamos el ancla.
    bd::XfvQueriedSenseDataGetInfo getInfo{};
    getInfo.type = bd::kTypeQueriedSenseDataGetInfo;
    getInfo.next = nullptr;

    struct CountProbe {
        XrStructureType type;
        void* next;
        uint32_t stateCapacityInput;
        uint32_t stateCountOutput;
        bd::XfvSpatialEntityState* states;
    } probe{};
    probe.type = bd::kTypeQueriedSenseData;
    probe.next = nullptr;
    probe.stateCapacityInput = 0;
    probe.stateCountOutput = 0;
    probe.states = nullptr;
    if (g.getQueried(completion.snapshot, &getInfo,
                     reinterpret_cast<bd::XfvQueriedSenseData*>(&probe)) != XR_SUCCESS ||
        probe.stateCountOutput == 0) {
        logWarn("El anchor persistido '%s' no aparece en el snapshot", etiqueta);
        return false;
    }
    uint32_t cap = probe.stateCountOutput;
    if (cap > 64) cap = 64;
    std::vector<bd::XfvSpatialEntityState> estados(cap);
    for (auto& e : estados) {
        e.type = bd::kTypeSpatialEntityState;
        e.next = nullptr;
        e.entityId = 0;
        e.lastUpdateTime = 0;
        memset(e.uuid.data, 0, sizeof(e.uuid.data));
    }
    bd::XfvQueriedSenseData queried{};
    queried.type = bd::kTypeQueriedSenseData;
    queried.next = nullptr;
    queried.stateCapacityInput = cap;
    queried.stateCountOutput = 0;
    queried.states = estados.data();
    if (g.getQueried(completion.snapshot, &getInfo, &queried) != XR_SUCCESS) {
        return false;
    }
    const uint32_t n = queried.stateCountOutput < cap ? queried.stateCountOutput : cap;
    for (uint32_t i = 0; i < n; ++i) {
        if (memcmp(estados[i].uuid.data, uuid.data, 16) != 0) continue;
        if (!crearAnclaSobreEntidad(g.provAnchors.handle, completion.snapshot, estados[i].entityId)) {
            return false;
        }
        g.estado = Estado::kAnclado;
        logInfo("Ancla '%s' restaurada", etiqueta);
        return true;
    }
    logWarn("El anchor persistido '%s' no se encontró", etiqueta);
    return false;
}

const char* ancla_espacial_estado() {
    switch (g.estado) {
        case Estado::kSinExtension: return "sin extension";
        case Estado::kInactivo: return "extension inactiva";
        case Estado::kListo: return "listo";
        case Estado::kEsperandoMesa: return "esperando mesa";
        case Estado::kAnclando: return "anclando";
        case Estado::kAnclado: return "anclado";
        case Estado::kPersistido: return "persistido";
        case Estado::kMesaPerdida: return "mesa perdida";
    }
    return "sin extension";
}

bool ancla_espacial_parametros(float* escala_casilla, float* altura_diorama) {
    if (escala_casilla) *escala_casilla = kEscalaCasillaM;
    if (altura_diorama) *altura_diorama = kAlturaDioramaM;
    return ancla_espacial_disponible();
}

void ancla_espacial_fijar_ruta_persistencia(const char* ruta) {
    g.rutaPersistencia = (ruta && *ruta) ? ruta : "";
    cargarTablaPersistencia();
}

bool ancla_espacial_posicion(float out_xyz[3]) {
    if (!out_xyz) return false;
    if (!ancla_espacial_disponible() || !g.anclaValida || !g.trackingOk) {
        out_xyz[0] = out_xyz[1] = out_xyz[2] = 0.0f;
        return false;
    }
    out_xyz[0] = g.poseAncla.position.x;
    out_xyz[1] = g.poseAncla.position.y;
    out_xyz[2] = g.poseAncla.position.z;
    return true;
}

float ancla_espacial_escala_casilla() { return kEscalaCasillaM; }

}  // namespace xfv
