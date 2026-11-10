package org.revival.graphics;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

/** Loads the common native renderer and exposes its copied packet transport. */
public final class NativeGraphics {
	public static final int ABI_VERSION = 1;
	public static final long FEATURE_SURFACE_COMMANDS = 1L;
	public static final long FEATURE_COMPLETED_FRAMES = 1L << 1;
	public static final long FEATURE_SCALE2X = 1L << 2;
	public static final long FEATURE_ACTUAL_DEVICE = 1L << 3;
	public static final long FEATURE_SURFACE_SCALE = 1L << 4;
	public static final int INPUT_MODE_COMMANDS = 1;
	public static final int FILTER_NEAREST = 1;
	public static final int FILTER_LINEAR = 2;
	public enum Result {
		OK(0), INVALID_ARGUMENT(1), UNSUPPORTED_ABI(2), BAD_PACKET(3), QUEUE_FULL(4),
		WRONG_THREAD(5), STALE_HANDLE(6), RESOURCE_LIMIT(7), NO_CONTEXT(8), GL_FAILURE(9),
		CLOSING(10), UNSUPPORTED_OPERATION(11), INTERNAL_ERROR(12);

		private final int code;
		Result(int code) { this.code = code; }
		private static Result fromCode(int code) {
			for (Result result : values()) if (result.code == code) return result;
			throw new IllegalStateException("Native graphics returned an unknown result code");
		}
	}

	private static Path loadedCore;
	private static Path loadedShim;
	private final BuildInfo buildInfo;

	public NativeGraphics(Path coreDll, Path jniShim) throws IOException {
		loadLibraries(coreDll, jniShim);
		this.buildInfo = decodeBuildInfo(nativeBuildInfo());
		if (buildInfo.abiVersion() != ABI_VERSION) {
			throw new UnsupportedOperationException("Native graphics ABI version is not supported");
		}
	}

	public BuildInfo buildInfo() {
		return buildInfo;
	}

	/** The native side copies these bytes before returning; the caller must not mutate them during the call. */
	public Result submitPacket(long context, ByteBuffer packet) {
		if (context == 0 || packet == null || !packet.isDirect()) {
			throw new IllegalArgumentException("A nonzero context and direct packet buffer are required");
		}
		int offset = packet.position();
		int length = packet.remaining();
		if (length < GraphicsPacketWriter.HEADER_BYTES ||
			(long) offset + length > packet.capacity()) {
			throw new IllegalArgumentException("Packet buffer view is outside its direct storage");
		}
		return Result.fromCode(nativeSubmitPacket(context, packet, offset, length));
	}

	public AttachResult attach(long epoch, long textureBudgetBytes, int maxQueuedBytes,
		int maxQueuedOps, int maxPendingFrames, int detailScale, int inputMode) {
		if (epoch == 0 || textureBudgetBytes <= 0 || maxQueuedBytes <= 0 || maxQueuedOps <= 0 ||
			maxPendingFrames < 1 || maxPendingFrames > 2 ||
			(detailScale != 2 && detailScale != 4) ||
			(inputMode != INPUT_MODE_COMMANDS && inputMode != 2)) {
			throw new IllegalArgumentException("Graphics context settings are outside the v1 bounds");
		}
		long[] result = nativeAttach(epoch, textureBudgetBytes, maxQueuedBytes,
			maxQueuedOps, maxPendingFrames, detailScale, inputMode);
		if (result == null || result.length != 2) {
			throw new IllegalStateException("Native graphics attach returned malformed status");
		}
		return new AttachResult(Result.fromCode((int) result[0]), result[1]);
	}

	public PumpResult pump(long context, long budgetNanos) {
		if (context == 0 || budgetNanos < 0) throw new IllegalArgumentException("Invalid context or pump budget");
		long[] result = nativePump(context, budgetNanos);
		if (result == null || result.length != 7) {
			throw new IllegalStateException("Native graphics pump returned malformed status");
		}
		return new PumpResult(Result.fromCode((int) result[0]), (int) result[1], (int) result[2],
			(int) result[3], (int) result[4], result[5], result[6]);
	}

	public Result present(long context, int width, int height, int aspectWidth,
		int aspectHeight, int filter) {
		if (context == 0 || width <= 0 || height <= 0 || aspectWidth <= 0 || aspectHeight <= 0 ||
			(filter != FILTER_NEAREST && filter != FILTER_LINEAR)) {
			throw new IllegalArgumentException("Invalid viewport settings");
		}
		return Result.fromCode(nativePresent(context, width, height, aspectWidth, aspectHeight, filter));
	}

	public Result clearFrame(long context) {
		if (context == 0) throw new IllegalArgumentException("Context handle is required");
		return Result.fromCode(nativeClearFrame(context));
	}

	/** Diagnostic-only surface readback. Normal gameplay never calls this path. */
	public byte[] readbackSurface(long context, long surface, int x, int y, int width, int height) {
		if (context == 0 || surface == 0 || x < 0 || y < 0 || width <= 0 || height <= 0 ||
			(long) width * height > 16L * 1024L * 1024L) {
			throw new IllegalArgumentException("Readback bounds are invalid");
		}
		int bytes = Math.multiplyExact(Math.multiplyExact(width, height), 4);
		ByteBuffer output = ByteBuffer.allocateDirect(bytes);
		Result result = Result.fromCode(nativeReadback(context, surface, x, y, width, height,
			GraphicsPacketWriter.PIXEL_FORMAT_RGBA8, GraphicsPacketWriter.ALPHA_PREMULTIPLIED, output));
		if (result != Result.OK) throw new IllegalStateException("Native graphics readback failed: " + result);
		byte[] copy = new byte[bytes];
		output.get(copy);
		return copy;
	}

	public Result beginClose(long context) {
		if (context == 0) throw new IllegalArgumentException("Context handle is required");
		return Result.fromCode(nativeBeginClose(context));
	}

	public Result destroyOnOwner(long context) {
		if (context == 0) throw new IllegalArgumentException("Context handle is required");
		return Result.fromCode(nativeDestroyOnOwner(context));
	}

	private static synchronized void loadLibraries(Path coreDll, Path jniShim) throws IOException {
		Path core = requireLibrary(coreDll);
		Path shim = requireLibrary(jniShim);
		if (loadedCore != null) {
			if (!loadedCore.equals(core) || !loadedShim.equals(shim)) {
				throw new IllegalStateException("This JVM already loaded a different graphics DLL pair");
			}
			return;
		}
		System.load(core.toString());
		System.load(shim.toString());
		loadedCore = core;
		loadedShim = shim;
	}

	private static Path requireLibrary(Path path) throws IOException {
		if (path == null) throw new IllegalArgumentException("Native library path is required");
		Path resolved = path.toRealPath();
		if (!Files.isRegularFile(resolved)) {
			throw new IOException("Native library path is not a file: " + resolved);
		}
		return resolved;
	}

	private static BuildInfo decodeBuildInfo(byte[] bytes) {
		if (bytes == null || bytes.length != 76) {
			throw new IllegalStateException("Native graphics build information has an invalid size");
		}
		ByteBuffer input = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN);
		int size = input.getInt();
		int abi = input.getInt();
		byte[] idBytes = new byte[64];
		input.get(idBytes);
		long features = Integer.toUnsignedLong(input.getInt());
		String id = new String(idBytes, StandardCharsets.US_ASCII);
		if (size != 76 || !id.matches("[0-9a-f]{64}")) {
			throw new IllegalStateException("Native graphics build information is malformed");
		}
		return new BuildInfo(abi, id, features);
	}

	private static native byte[] nativeBuildInfo();
	private static native int nativeSubmitPacket(long context, ByteBuffer packet, int offset, int length);
	private static native long[] nativeAttach(long epoch, long textureBudgetBytes, int maxQueuedBytes,
		int maxQueuedOps, int maxPendingFrames, int detailScale, int inputMode);
	private static native long[] nativePump(long context, long budgetNanos);
	private static native int nativePresent(long context, int width, int height,
		int aspectWidth, int aspectHeight, int filter);
	private static native int nativeClearFrame(long context);
	private static native int nativeReadback(long context, long surface, int x, int y,
		int width, int height, int outputFormat, int outputAlphaMode, ByteBuffer destination);
	private static native int nativeBeginClose(long context);
	private static native int nativeDestroyOnOwner(long context);

	public static final class AttachResult {
		private final Result result;
		private final long context;
		private AttachResult(Result result, long context) { this.result = result; this.context = context; }
		public Result result() { return result; }
		public long context() { return context; }
	}

	public static final class PumpResult {
		private final Result result;
		private final int operationsProcessed;
		private final int framesCompleted;
		private final int queuedBytes;
		private final int queuedOperations;
		private final long uploadedBytes;
		private final long elapsedNanos;
		private PumpResult(Result result, int operationsProcessed, int framesCompleted,
			int queuedBytes, int queuedOperations, long uploadedBytes, long elapsedNanos) {
			this.result = result;
			this.operationsProcessed = operationsProcessed;
			this.framesCompleted = framesCompleted;
			this.queuedBytes = queuedBytes;
			this.queuedOperations = queuedOperations;
			this.uploadedBytes = uploadedBytes;
			this.elapsedNanos = elapsedNanos;
		}
		public Result result() { return result; }
		public int operationsProcessed() { return operationsProcessed; }
		public int framesCompleted() { return framesCompleted; }
		public int queuedBytes() { return queuedBytes; }
		public int queuedOperations() { return queuedOperations; }
		public long uploadedBytes() { return uploadedBytes; }
		public long elapsedNanos() { return elapsedNanos; }
	}

	public static final class BuildInfo {
		private final int abiVersion;
		private final String buildId;
		private final long featureBits;
		private BuildInfo(int abiVersion, String buildId, long featureBits) {
			this.abiVersion = abiVersion;
			this.buildId = buildId;
			this.featureBits = featureBits;
		}
		public int abiVersion() { return abiVersion; }
		public String buildId() { return buildId; }
		public long featureBits() { return featureBits; }
	}
}
