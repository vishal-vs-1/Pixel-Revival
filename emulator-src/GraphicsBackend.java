/* Renderer boundary for the Java host. GPL-3.0-or-later. */
package org.recompile.mobile;

import java.awt.image.BufferedImage;

/**
 * Owns one renderer implementation while keeping game semantics in Enhancement.
 * Implementations are called by the game thread for semantic work and by the
 * window thread for pump/present/cleanup work.
 */
public interface GraphicsBackend {
    String name();
    boolean common();
    String device();
    long submittedFrames();

    void observe(PlatformImage image);
    boolean stageReplacement(PlatformImage image, BufferedImage texture);
    void derive(PlatformImage image, PlatformImage source, int x, int y, int w, int h, int transform);
    void pixel(PlatformImage image, int index, int value);
    void textPatch(PlatformGraphics graphics, int x, int y, int w, int h, int[] pixels, int rasterScale);
    void fill(PlatformGraphics graphics, int x, int y, int w, int h);
    void blit(PlatformGraphics graphics, PlatformImage image, int sx, int sy, int sw, int sh,
              int x, int y, int dw, int dh, int transform, boolean flush);
    void sync(PlatformImage image, int x, int y, int w, int h);
    void publish(PlatformImage image);

    int pump(long budgetNanos);
    void present(int width, int height);
    void cleanup();
    BufferedImage readback(PlatformImage image);
    default boolean readbackSupported() { return true; }
    default void stopFramePublication() {}
    void shutdown();

    boolean warmupScaler();
    long scalerPasses();
    long scalerFallbacks();
    long scalerSubmitNanos();
}
