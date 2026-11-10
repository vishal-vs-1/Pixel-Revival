#include <stddef.h>
#include <stdint.h>

#include "revival_graphics.h"

_Static_assert(sizeof(RvContextHandle) == 8, "context handle must remain 64-bit");
_Static_assert(sizeof(RvSurfaceHandle) == 8, "surface handle must remain 64-bit");
_Static_assert(sizeof(RvBuildInfo) == RV_BUILD_INFO_BYTES, "C header layout mismatch");
_Static_assert(offsetof(RvBuildInfo, feature_bits) == 72, "C header field offset mismatch");
_Static_assert(sizeof(RvAttachConfig) == RV_ATTACH_CONFIG_BYTES, "C header layout mismatch");
_Static_assert(offsetof(RvAttachConfig, session_epoch) == 8, "C header field offset mismatch");
_Static_assert(sizeof(RvFrameDesc) == RV_FRAME_DESC_BYTES, "C header layout mismatch");
_Static_assert(offsetof(RvFrameDesc, frame_sequence) == 8, "C header field offset mismatch");
_Static_assert(sizeof(RvViewport) == RV_VIEWPORT_BYTES, "C header layout mismatch");
_Static_assert(sizeof(RvReadbackRect) == RV_READBACK_RECT_BYTES, "C header layout mismatch");
_Static_assert(sizeof(RvPumpStats) == RV_PUMP_STATS_BYTES, "C header layout mismatch");
_Static_assert(sizeof(RvStats) == RV_STATS_BYTES, "C header layout mismatch");
