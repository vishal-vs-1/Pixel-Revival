package org.recompile.mobile;

import org.revival.graphics.NativeGraphics;

/** Pure rollout guard check. It does not load a native library or create a window. */
public final class GraphicsBackendSelectionCheck {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    public static void main(String[] args) {
        check("legacy-java".equals(new LegacyGraphicsBackend().name()), "legacy adapter identity changed");
        check(!new LegacyGraphicsBackend().common(), "legacy adapter marked common");
        check(!GpuRenderer.commonSelected(), "common backend selected without an explicit player selection");
        check("legacy-java".equals(GpuRenderer.backendName()), "legacy backend is not the default facade");
        check(NativeGraphics.FEATURE_SURFACE_SCALE == (1L << 4), "surface-scale feature bit changed");
        System.setProperty("jme.graphicsBackend", "common-v1");
        try {
            check(CommonGraphicsBackend.tryCreate(176, 220, 4, "test") == null,
                    "common backend did not fail closed without verified native paths");
        } finally { System.clearProperty("jme.graphicsBackend"); }
        System.out.println("Graphics backend rollout guard passed");
    }
}
