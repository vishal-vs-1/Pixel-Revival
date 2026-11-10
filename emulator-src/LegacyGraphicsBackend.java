/* Legacy Java/LWJGL renderer adapter. GPL-3.0-or-later. */
package org.recompile.mobile;

import java.awt.image.BufferedImage;

/**
 * Compatibility adapter for the existing renderer. It deliberately delegates
 * to the unchanged GpuRenderer implementation and is never selected as the
 * active backend, which prevents recursive facade calls during rollout.
 */
public final class LegacyGraphicsBackend implements GraphicsBackend {
    @Override public String name() { return "legacy-java"; }
    @Override public boolean common() { return false; }
    @Override public String device() { return GpuRenderer.device; }
    @Override public long submittedFrames() { return GpuRenderer.submittedFrames; }
    @Override public void observe(PlatformImage image) { GpuRenderer.observe(image); }
    @Override public boolean stageReplacement(PlatformImage image, BufferedImage texture) {
        return GpuRenderer.stageReplacement(image, texture);
    }
    @Override public void derive(PlatformImage image, PlatformImage source, int x, int y, int w, int h, int transform) {
        GpuRenderer.derive(image, source, x, y, w, h, transform);
    }
    @Override public void pixel(PlatformImage image, int index, int value) { GpuRenderer.pixel(image, index, value); }
    @Override public void textPatch(PlatformGraphics graphics, int x, int y, int w, int h, int[] pixels, int rasterScale) {
        GpuRenderer.textPatch(graphics, x, y, w, h, pixels, rasterScale);
    }
    @Override public void fill(PlatformGraphics graphics, int x, int y, int w, int h) {
        GpuRenderer.fill(graphics, x, y, w, h);
    }
    @Override public void blit(PlatformGraphics graphics, PlatformImage image, int sx, int sy, int sw, int sh,
                              int x, int y, int dw, int dh, int transform, boolean flush) {
        GpuRenderer.blit(graphics, image, sx, sy, sw, sh, x, y, dw, dh, transform, flush);
    }
    @Override public void sync(PlatformImage image, int x, int y, int w, int h) { GpuRenderer.sync(image, x, y, w, h); }
    @Override public void publish(PlatformImage image) { GpuRenderer.publish(image); }
    @Override public int pump(long budgetNanos) { return GpuRenderer.pump(budgetNanos); }
    @Override public void present(int width, int height) { GpuRenderer.present(width, height); }
    @Override public void cleanup() { GpuRenderer.cleanup(); }
    @Override public BufferedImage readback(PlatformImage image) { return GpuRenderer.readback(image); }
    @Override public void shutdown() { GpuRenderer.shutdown(); }
    @Override public boolean warmupScaler() { return GpuRenderer.warmupFastScaler(); }
    @Override public long scalerPasses() { return GpuRenderer.fastScalerPasses(); }
    @Override public long scalerFallbacks() { return GpuRenderer.fastScalerFallbacks(); }
    @Override public long scalerSubmitNanos() { return GpuRenderer.fastScalerSubmitNanos(); }
}
