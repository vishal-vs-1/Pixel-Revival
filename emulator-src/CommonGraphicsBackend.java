/* Opt-in common native graphics adapter. GPL-3.0-or-later. */
package org.recompile.mobile;

import java.awt.Rectangle;
import java.awt.image.BufferedImage;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Paths;
import java.util.Map;
import java.util.WeakHashMap;
import org.revival.graphics.GraphicsPacketWriter;
import org.revival.graphics.NativeGraphics;

/**
 * Converts Java image operations into bounded packets for the shared native
 * renderer. It is created only after an explicit common-v1 opt-in and a live
 * owner OpenGL context has already been established by GpuPlayer.
 */
public final class CommonGraphicsBackend implements GraphicsBackend {
    private static final String REQUESTED = "common-v1";
    private static final long TEXTURE_BUDGET = 256L * 1024L * 1024L;
    private static final int QUEUED_BYTES = 8 * 1024 * 1024;
    private static final int QUEUED_OPS = 8192;
    private static final int MAX_UPLOAD_BYTES = 1024 * 1024;
    private static final int PENDING_FRAMES = 2;

    private final NativeGraphics nativeGraphics;
    private final GraphicsPacketSink packetSink;
    private final long context;
    private final long sessionEpoch;
    private final int scale;
    private final int aspectWidth;
    private final int aspectHeight;
    private final String device;
    private final FrameBackpressure frames = new FrameBackpressure(PENDING_FRAMES);
    private final Map<PlatformImage, SurfaceState> surfaces = new WeakHashMap<PlatformImage, SurfaceState>();
    private long packetSequence = 1;
    private long nextSurface = 1;
    private long frameSequence = 1;
    private volatile boolean closed;
    private volatile long submitted;

    private static final class SurfaceState {
        final long source;
        final long destination;
        final int width;
        final int height;
        BufferedImage replacement;
        SurfaceState(long source, long destination, int width, int height) {
            this.source = source;
            this.destination = destination;
            this.width = width;
            this.height = height;
        }
    }

    private CommonGraphicsBackend(NativeGraphics nativeGraphics, long context, long sessionEpoch,
                                  int logicalWidth, int logicalHeight, int scale, String device) {
        this(new GraphicsPacketSink() {
            @Override public NativeGraphics.Result submit(long handle, ByteBuffer packet) {
                return nativeGraphics.submitPacket(handle, packet);
            }
        }, nativeGraphics, context, sessionEpoch, logicalWidth, logicalHeight, scale, device);
    }

    // Package-private for CPU-only semantic fixtures. The sink owns no GL state.
    CommonGraphicsBackend(GraphicsPacketSink packetSink, long context, long sessionEpoch,
                          int logicalWidth, int logicalHeight, int scale, String device) {
        this(packetSink, null, context, sessionEpoch, logicalWidth, logicalHeight, scale, device);
    }

    private CommonGraphicsBackend(GraphicsPacketSink packetSink, NativeGraphics nativeGraphics,
                                  long context, long sessionEpoch, int logicalWidth, int logicalHeight,
                                  int scale, String device) {
        this.packetSink = packetSink;
        this.nativeGraphics = nativeGraphics;
        this.context = context;
        this.sessionEpoch = sessionEpoch;
        this.aspectWidth = logicalWidth;
        this.aspectHeight = logicalHeight;
        this.scale = scale;
        this.device = device == null ? "unknown" : device;
    }

    /** Returns null when common mode was not explicitly requested or cannot be verified. */
    public static CommonGraphicsBackend tryCreate(int logicalWidth, int logicalHeight, int scale, String device) {
        if (!REQUESTED.equals(System.getProperty("jme.graphicsBackend", "legacy"))) return null;
        if (logicalWidth <= 0 || logicalHeight <= 0 || (scale != 2 && scale != 4)) {
            System.err.println("Common graphics unavailable: detail scale must be 2x or 4x");
            return null;
        }
        String core = System.getProperty("jme.graphicsCore", "");
        String shim = System.getProperty("jme.graphicsShim", "");
        if (core.length() == 0 || shim.length() == 0) {
            System.err.println("Common graphics unavailable: jme.graphicsCore and jme.graphicsShim are required");
            return null;
        }
        try {
            NativeGraphics graphics = new NativeGraphics(Paths.get(core), Paths.get(shim));
            NativeGraphics.BuildInfo info = graphics.buildInfo();
            long required = NativeGraphics.FEATURE_SURFACE_COMMANDS |
                    NativeGraphics.FEATURE_SCALE2X | NativeGraphics.FEATURE_ACTUAL_DEVICE |
                    NativeGraphics.FEATURE_SURFACE_SCALE;
            if ((info.featureBits() & required) != required) {
                throw new IllegalStateException("common DLL lacks required surface or scaling capabilities");
            }
            long epoch = System.nanoTime();
            NativeGraphics.AttachResult attached = graphics.attach(epoch, TEXTURE_BUDGET,
                    QUEUED_BYTES, QUEUED_OPS, PENDING_FRAMES, scale, NativeGraphics.INPUT_MODE_COMMANDS);
            if (attached.result() != NativeGraphics.Result.OK || attached.context() == 0) {
                throw new IllegalStateException("native context attach failed: " + attached.result());
            }
            return new CommonGraphicsBackend(graphics, attached.context(), epoch,
                    logicalWidth, logicalHeight, scale, device);
        } catch (UnsatisfiedLinkError failure) {
            System.err.println("Common graphics unavailable: native library load failed: " + failure.getMessage());
            return null;
        } catch (Exception failure) {
            System.err.println("Common graphics unavailable: " + failure.getMessage());
            return null;
        }
    }

    @Override public String name() { return REQUESTED; }
    @Override public boolean common() { return true; }
    @Override public String device() { return device; }
    @Override public long submittedFrames() { return submitted; }

    private synchronized SurfaceState ensureSurface(PlatformImage image) {
        if (closed) throw new IllegalStateException("common graphics backend is closed");
        if (image == null) throw new IllegalArgumentException("image is required");
        SurfaceState state = surfaces.get(image);
        if (state != null) return state;
        int width = image.getWidth(), height = image.getHeight();
        if (width <= 0 || height <= 0) throw new IllegalArgumentException("image dimensions are invalid");
        long source = nextSurface++, destination = nextSurface++;
        state = new SurfaceState(source, destination, width, height);
        submit(new GraphicsPacketWriter(sessionEpoch, nextPacket()).createSurface(source, width, height, 1)
                .createSurface(destination, width, height, scale).buildDirect());
        surfaces.put(image, state);
        uploadOriginal(image, state);
        return state;
    }

    private long nextPacket() { return packetSequence++; }

    private void submit(ByteBuffer packet) {
        NativeGraphics.Result result = packetSink.submit(context, packet);
        if (result != NativeGraphics.Result.OK) throw new IllegalStateException("native graphics submit failed: " + result);
    }

    private static int[] snapshot(PlatformImage image) {
        synchronized (image) {
            BufferedImage canvas = image.getCanvas();
            return canvas.getRGB(0, 0, canvas.getWidth(), canvas.getHeight(), null, 0, canvas.getWidth());
        }
    }

    private static byte[] rgbaBytes(int[] pixels) {
        byte[] bytes = new byte[pixels.length * 4];
        ByteBuffer output = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN);
        for (int argb : pixels) {
            int alpha = (argb >>> 24) & 255;
            int red = (((argb >>> 16) & 255) * alpha + 127) / 255;
            int green = (((argb >>> 8) & 255) * alpha + 127) / 255;
            int blue = ((argb & 255) * alpha + 127) / 255;
            output.put((byte) red).put((byte) green).put((byte) blue).put((byte) alpha);
        }
        return bytes;
    }

    private void uploadOriginal(PlatformImage image, SurfaceState state) {
        uploadSurface(state.source, state.width, state.height, 1, snapshot(image));
        submit(new GraphicsPacketWriter(sessionEpoch, nextPacket()).scaleSurface(state.source, state.destination, scale).buildDirect());
    }

    private void uploadSurface(long surface, int width, int height, int storageScale, int[] pixels) {
        byte[] bytes = rgbaBytes(pixels);
        int physicalWidth = width * storageScale;
        int physicalHeight = height * storageScale;
        int stride = physicalWidth * 4;
        GraphicsPacketWriter.FrameDescriptor descriptor = new GraphicsPacketWriter.FrameDescriptor(
                0, physicalWidth, physicalHeight, stride, width, height, storageScale, storageScale,
                GraphicsPacketWriter.PIXEL_FORMAT_RGBA8, GraphicsPacketWriter.ALPHA_PREMULTIPLIED, 1);
        // The caller supplies pixels at the declared physical density.
        GraphicsPacketWriter writer = new GraphicsPacketWriter(sessionEpoch, nextPacket()).beginUpload(surface, descriptor, bytes.length);
        int rowsPerPacket = Math.max(1, MAX_UPLOAD_BYTES / stride);
        int row = 0;
        while (row < physicalHeight) {
            int count = Math.min(rowsPerPacket, physicalHeight - row);
            byte[] rows = new byte[count * stride];
            System.arraycopy(bytes, row * stride, rows, 0, rows.length);
            writer.uploadRows(surface, row, count, stride, rows);
            row += count;
            if (row < physicalHeight) {
                submit(writer.buildDirect());
                writer = new GraphicsPacketWriter(sessionEpoch, nextPacket());
            }
        }
        writer.endUpload(surface);
        submit(writer.buildDirect());
    }

    private void uploadTexture(PlatformImage image, SurfaceState state, BufferedImage texture) {
        if (texture.getWidth() != state.width * scale || texture.getHeight() != state.height * scale) {
            throw new IllegalArgumentException("replacement texture dimensions do not match the logical image");
        }
        int[] pixels = texture.getRGB(0, 0, texture.getWidth(), texture.getHeight(), null, 0, texture.getWidth());
        uploadSurface(state.destination, state.width, state.height, scale, pixels);
    }

    private void fillRect(long destination, int x, int y, int width, int height, int color, Rectangle clip) {
        GraphicsPacketWriter.Rect target = new GraphicsPacketWriter.Rect(x, y, width, height);
        GraphicsPacketWriter.Rect bounds = new GraphicsPacketWriter.Rect(clip.x, clip.y, clip.width, clip.height);
        submit(new GraphicsPacketWriter(sessionEpoch, nextPacket()).fill(destination, target, color, bounds).buildDirect());
    }

    @Override public synchronized void observe(PlatformImage image) {
        if (closed) return;
        SurfaceState state = ensureSurface(image);
        BufferedImage replacement = image.hdCanvas;
        if (replacement != null && replacement != state.replacement) {
            uploadTexture(image, state, replacement);
            state.replacement = replacement;
        }
    }

    @Override public synchronized boolean stageReplacement(PlatformImage image, BufferedImage texture) {
        if (closed || image == null || texture == null) return false;
        try {
            SurfaceState state = ensureSurface(image);
            uploadTexture(image, state, texture);
            state.replacement = texture;
            return true;
        }
        catch (RuntimeException failure) { System.err.println("Common replacement rejected: " + failure.getMessage()); return false; }
    }

    @Override public synchronized void derive(PlatformImage image, PlatformImage source, int x, int y, int w, int h, int transform) {
        if (closed) return;
        SurfaceState src = ensureSurface(source), dst = ensureSurface(image);
        GraphicsPacketWriter.Rect sourceRect = new GraphicsPacketWriter.Rect(x, y, w, h);
        GraphicsPacketWriter.Rect destinationRect = new GraphicsPacketWriter.Rect(0, 0, w, h);
        GraphicsPacketWriter.Rect clip = new GraphicsPacketWriter.Rect(0, 0, dst.width, dst.height);
        submit(new GraphicsPacketWriter(sessionEpoch, nextPacket()).blit(src.destination, dst.destination, sourceRect,
                destinationRect, clip, transform, GraphicsPacketWriter.BLEND_REPLACE).buildDirect());
    }

    @Override public synchronized void pixel(PlatformImage image, int index, int value) {
        if (closed) return;
        SurfaceState state = ensureSurface(image);
        if (index < 0 || index >= state.width * state.height) return;
        int x = index % state.width, y = index / state.width;
        fillRect(state.destination, x, y, 1, 1, value,
                new Rectangle(0, 0, state.width, state.height));
    }

    @Override public synchronized void textPatch(PlatformGraphics graphics, int x, int y, int w, int h,
                                                   int[] pixels, int rasterScale) {
        if (closed) return;
        if (graphics == null || pixels == null || w <= 0 || h <= 0 || (rasterScale != 1 && rasterScale != scale)) return;
        SurfaceState dst = ensureSurface(graphics.baseImage);
        long temporary = nextSurface++;
        submit(new GraphicsPacketWriter(sessionEpoch, nextPacket()).createSurface(temporary, w, h, rasterScale).buildDirect());
        uploadSurface(temporary, w, h, rasterScale, pixels);
        Rectangle clip = graphics.gc.getClipBounds();
        GraphicsPacketWriter.Rect destination = new GraphicsPacketWriter.Rect(x + graphics.translateX, y + graphics.translateY, w, h);
        GraphicsPacketWriter.Rect source = new GraphicsPacketWriter.Rect(0, 0, w, h);
        GraphicsPacketWriter.Rect bounds = new GraphicsPacketWriter.Rect(clip.x + graphics.translateX,
                clip.y + graphics.translateY, clip.width, clip.height);
        submit(new GraphicsPacketWriter(sessionEpoch, nextPacket()).blit(temporary, dst.destination, source, destination, bounds,
                0, GraphicsPacketWriter.BLEND_SOURCE_OVER).destroySurface(temporary).buildDirect());
    }

    @Override public synchronized void fill(PlatformGraphics graphics, int x, int y, int w, int h) {
        if (closed) return;
        if (graphics == null || w <= 0 || h <= 0) return;
        SurfaceState dst = ensureSurface(graphics.baseImage);
        Rectangle clip = graphics.gc.getClipBounds();
        fillRect(dst.destination, x + graphics.translateX, y + graphics.translateY, w, h, graphics.color,
                new Rectangle(clip.x + graphics.translateX, clip.y + graphics.translateY, clip.width, clip.height));
    }

    @Override public synchronized void blit(PlatformGraphics graphics, PlatformImage image, int sx, int sy, int sw,
                                             int sh, int x, int y, int dw, int dh, int transform, boolean flush) {
        if (closed) return;
        if (graphics == null || image == null || sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0) return;
        SurfaceState src = ensureSurface(image), dst = ensureSurface(graphics.baseImage);
        if (sx < 0 || sy < 0 || sx + sw > src.width || sy + sh > src.height) return;
        Rectangle clip = flush ? new Rectangle(0, 0, dst.width, dst.height) : graphics.gc.getClipBounds();
        int dx = x + (flush ? 0 : graphics.translateX), dy = y + (flush ? 0 : graphics.translateY);
        GraphicsPacketWriter.Rect source = new GraphicsPacketWriter.Rect(sx, sy, sw, sh);
        GraphicsPacketWriter.Rect destination = new GraphicsPacketWriter.Rect(dx, dy, dw, dh);
        GraphicsPacketWriter.Rect bounds = new GraphicsPacketWriter.Rect(clip.x + (flush ? 0 : graphics.translateX),
                clip.y + (flush ? 0 : graphics.translateY), clip.width, clip.height);
        submit(new GraphicsPacketWriter(sessionEpoch, nextPacket()).blit(src.destination, dst.destination, source, destination, bounds,
                transform, flush ? GraphicsPacketWriter.BLEND_REPLACE : GraphicsPacketWriter.BLEND_SOURCE_OVER).buildDirect());
    }

    @Override public synchronized void sync(PlatformImage image, int x, int y, int w, int h) {
        if (closed) return;
        SurfaceState state = ensureSurface(image);
        uploadOriginal(image, state);
    }

    @Override public void publish(PlatformImage image) {
        // Wait outside the surface lock. The native queue counts both pending
        // and in-flight commits, so a third frame must wait for an owner pump.
        if (closed || !frames.acquire()) return;
        boolean accepted = false;
        try {
            synchronized (this) {
                if (closed) return;
                SurfaceState state = ensureSurface(image);
                submit(new GraphicsPacketWriter(sessionEpoch, nextPacket()).commitFrame(state.destination, frameSequence++, scale,
                        new GraphicsPacketWriter.Rect(0, 0, state.width, state.height)).buildDirect());
                accepted = true;
                submitted++;
            }
        } finally {
            if (!accepted) frames.complete();
        }
    }

    void completeFrames(int completed) {
        for (int i = 0; i < completed; i++) frames.complete();
    }

    @Override public int pump(long budgetNanos) {
        if (closed) return 0;
        if (nativeGraphics == null) throw new UnsupportedOperationException("packet-only backend has no GL pump");
        NativeGraphics.PumpResult result = nativeGraphics.pump(context, budgetNanos);
        completeFrames(result.framesCompleted());
        if (result.result() != NativeGraphics.Result.OK) throw new IllegalStateException("native graphics pump failed: " + result.result());
        return result.operationsProcessed();
    }

    @Override public void present(int width, int height) {
        if (nativeGraphics == null) throw new UnsupportedOperationException("packet-only backend has no GL presentation");
        NativeGraphics.Result result = nativeGraphics.present(context, width, height, aspectWidth, aspectHeight,
                NativeGraphics.FILTER_LINEAR);
        if (result != NativeGraphics.Result.OK) throw new IllegalStateException("native graphics present failed: " + result);
    }

    @Override public synchronized void cleanup() { /* Native resources are retired by the bounded context. */ }

    @Override public BufferedImage readback(PlatformImage image) {
        if (nativeGraphics == null) throw new UnsupportedOperationException("packet-only backend has no diagnostic readback");
        SurfaceState state;
        synchronized (this) {
            if (closed) throw new IllegalStateException("common graphics backend is closed");
            state = surfaces.get(image);
        }
        if (state == null) throw new IllegalArgumentException("Image has no common graphics surface");
        int width = state.width * scale, height = state.height * scale;
        byte[] bytes = nativeGraphics.readbackSurface(context, state.destination, 0, 0, width, height);
        BufferedImage result = new BufferedImage(width, height, BufferedImage.TYPE_INT_ARGB);
        for (int y = 0; y < height; y++) for (int x = 0; x < width; x++) {
            int offset = (y * width + x) * 4;
            int red = bytes[offset] & 255, green = bytes[offset + 1] & 255;
            int blue = bytes[offset + 2] & 255, alpha = bytes[offset + 3] & 255;
            if (alpha > 0 && alpha < 255) {
                red = Math.min(255, (red * 255 + alpha / 2) / alpha);
                green = Math.min(255, (green * 255 + alpha / 2) / alpha);
                blue = Math.min(255, (blue * 255 + alpha / 2) / alpha);
            }
            result.setRGB(x, y, (alpha << 24) | (red << 16) | (green << 8) | blue);
        }
        return result;
    }
    @Override public boolean readbackSupported() { return nativeGraphics != null; }

    @Override public void stopFramePublication() { frames.close(); }

    @Override public synchronized void shutdown() {
        if (closed) return;
        closed = true;
        frames.close();
        if (nativeGraphics == null) { surfaces.clear(); return; }
        try {
            nativeGraphics.beginClose(context);
            for (int i = 0; i < 32; i++) {
                NativeGraphics.PumpResult result = nativeGraphics.pump(context, 1_000_000L);
                if (result.result() != NativeGraphics.Result.OK || result.operationsProcessed() == 0) break;
            }
            nativeGraphics.destroyOnOwner(context);
        } finally { surfaces.clear(); }
    }

    @Override public boolean warmupScaler() { return true; }
    @Override public long scalerPasses() { return 0; }
    @Override public long scalerFallbacks() { return 0; }
    @Override public long scalerSubmitNanos() { return 0; }
}
