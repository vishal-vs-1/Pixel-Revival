package org.revival.graphics;

import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.ArrayList;
import java.util.List;

/** Encodes immutable little-endian v1 command packets for the shared native renderer. */
public final class GraphicsPacketWriter {
	public static final int HEADER_BYTES = 32;
	public static final int COMMAND_HEADER_BYTES = 8;
	public static final int MAX_PACKET_BYTES = 8 * 1024 * 1024;
	public static final int MAX_COMMANDS = 8192;
	public static final int PIXEL_FORMAT_RGBA8 = 1;
	public static final int PIXEL_FORMAT_BGRA8 = 2;
	public static final int ALPHA_OPAQUE = 1;
	public static final int ALPHA_STRAIGHT = 2;
	public static final int ALPHA_PREMULTIPLIED = 3;
	public static final int SCALE_ORIGINAL = 1;
	public static final int SCALE_2X = 2;
	public static final int SCALE_4X = 4;
	public static final int BLEND_REPLACE = 1;
	public static final int BLEND_SOURCE_OVER = 2;

	private static final int MAGIC = 0x31475652;
	private static final int ABI_VERSION = 1;
	private static final long MAX_LOGICAL_PIXELS = 4_194_304L;
	private static final long MAX_PIXEL_BYTES = 64L * 1024 * 1024;

	private final long sessionEpoch;
	private final long packetSequence;
	private final List<Command> commands = new ArrayList<>();
	private long encodedCommandBytes;

	public GraphicsPacketWriter(long sessionEpoch, long packetSequence) {
		if (sessionEpoch == 0 || packetSequence == 0) {
			throw new IllegalArgumentException("session epoch and packet sequence must be nonzero");
		}
		this.sessionEpoch = sessionEpoch;
		this.packetSequence = packetSequence;
	}

	public GraphicsPacketWriter createSurface(long surface, int logicalWidth,
		int logicalHeight, int storageScale) {
		if (surface <= 0 || logicalWidth <= 0 || logicalHeight <= 0 ||
			(long) logicalWidth * logicalHeight > MAX_LOGICAL_PIXELS || !validScale(storageScale)) {
			throw new IllegalArgumentException("surface dimensions, handle or scale are invalid");
		}
		ByteBuffer payload = littleEndian(20);
		payload.putLong(surface).putInt(logicalWidth).putInt(logicalHeight).putInt(storageScale);
		return add(1, payload.array());
	}

	public GraphicsPacketWriter beginUpload(long surface, FrameDescriptor descriptor,
		int totalPixelBytes) {
		requireSurface(surface);
		validateDescriptor(descriptor, totalPixelBytes, true);
		ByteBuffer payload = littleEndian(68);
		payload.putLong(surface);
		putDescriptor(payload, descriptor);
		payload.putInt(totalPixelBytes);
		return add(2, payload.array());
	}

	public GraphicsPacketWriter uploadRows(long surface, int firstRow, int rowCount,
		int sourceStrideBytes, byte[] rowPixels) {
		requireSurface(surface);
		if (firstRow < 0 || rowCount <= 0 || sourceStrideBytes <= 0 || rowPixels == null ||
			(long) rowCount * sourceStrideBytes != rowPixels.length) {
			throw new IllegalArgumentException("row range, stride and byte count do not agree");
		}
		requireCommandFits(24L + rowPixels.length);
		ByteBuffer payload = littleEndian(Math.addExact(24, rowPixels.length));
		payload.putLong(surface).putInt(firstRow).putInt(rowCount).putInt(sourceStrideBytes)
			.putInt(rowPixels.length).put(rowPixels);
		return add(3, payload.array());
	}

	public GraphicsPacketWriter endUpload(long surface) {
		requireSurface(surface);
		ByteBuffer payload = littleEndian(8).putLong(surface);
		return add(4, payload.array());
	}

	public GraphicsPacketWriter fill(long destination, Rect rectangle, int argb, Rect clip) {
		requireSurface(destination);
		requirePositive(rectangle, "fill rectangle");
		requireClip(clip);
		ByteBuffer payload = littleEndian(44).putLong(destination);
		putRect(payload, rectangle);
		payload.putInt(argb);
		putRect(payload, clip);
		return add(5, payload.array());
	}

	public GraphicsPacketWriter blit(long source, long destination, Rect sourceRect,
		Rect destinationRect, Rect clip, int transform, int blend) {
		requireSurface(source);
		requireSurface(destination);
		requirePositive(sourceRect, "source rectangle");
		requirePositive(destinationRect, "destination rectangle");
		requireClip(clip);
		if (transform < 0 || transform > 7 ||
			(blend != BLEND_REPLACE && blend != BLEND_SOURCE_OVER)) {
			throw new IllegalArgumentException("transform or blend mode is invalid");
		}
		ByteBuffer payload = littleEndian(72).putLong(source).putLong(destination);
		putRect(payload, sourceRect);
		putRect(payload, destinationRect);
		putRect(payload, clip);
		payload.putInt(transform).putInt(blend);
		return add(6, payload.array());
	}

	public GraphicsPacketWriter copy(long source, long destination, Rect sourceRect,
		Rect destinationRect, Rect clip, int transform, int blend) {
		ByteBuffer encoded = blitPayload(source, destination, sourceRect, destinationRect,
			clip, transform, blend);
		return add(7, encoded.array());
	}

	public GraphicsPacketWriter destroySurface(long surface) {
		requireSurface(surface);
		return add(8, littleEndian(8).putLong(surface).array());
	}

	public GraphicsPacketWriter commitFrame(long surface, long frameSequence,
		int desiredScale, Rect logicalContent) {
		requireSurface(surface);
		requirePositive(logicalContent, "frame content rectangle");
		if(logicalContent.x() < 0 || logicalContent.y() < 0 || frameSequence == 0 ||
			!validScale(desiredScale)) {
			throw new IllegalArgumentException("frame sequence, content origin or scale is invalid");
		}
		ByteBuffer payload = littleEndian(36).putLong(surface).putLong(frameSequence)
			.putInt(desiredScale);
		putRect(payload, logicalContent);
		return add(9, payload.array());
	}

	public GraphicsPacketWriter clearFrame() {
		return add(10, new byte[0]);
	}

	public GraphicsPacketWriter scaleSurface(long source, long destination, int targetScale) {
		requireSurface(source);
		requireSurface(destination);
		if (targetScale != SCALE_2X && targetScale != SCALE_4X) {
			throw new IllegalArgumentException("surface scale must be 2x or 4x");
		}
		ByteBuffer payload = littleEndian(24).putLong(source).putLong(destination)
			.putInt(targetScale).putInt(0);
		return add(11, payload.array());
	}

	public ByteBuffer buildDirect() {
		if (commands.size() > MAX_COMMANDS) {
			throw new IllegalStateException("packet has too many commands");
		}
		long total = HEADER_BYTES;
		for (Command command : commands) {
			total += COMMAND_HEADER_BYTES + (long) command.payload.length;
			if (total > MAX_PACKET_BYTES) {
				throw new IllegalStateException("packet exceeds the v1 byte limit");
			}
		}
		ByteBuffer packet = ByteBuffer.allocateDirect((int) total).order(ByteOrder.LITTLE_ENDIAN);
		packet.putInt(MAGIC).putShort((short) ABI_VERSION).putShort((short) HEADER_BYTES)
			.putInt((int) total).putInt(commands.size()).putLong(sessionEpoch).putLong(packetSequence);
		for (Command command : commands) {
			packet.putShort((short) command.opcode).putShort((short) 0)
				.putInt(COMMAND_HEADER_BYTES + command.payload.length).put(command.payload);
		}
		packet.flip();
		return packet;
	}

	private GraphicsPacketWriter add(int opcode, byte[] payload) {
		if (commands.size() == MAX_COMMANDS) {
			throw new IllegalStateException("packet has too many commands");
		}
		requireCommandFits(payload.length);
		// Payload arrays are created here; upload rows already copy their caller-owned bytes into one.
		commands.add(new Command(opcode, payload));
		encodedCommandBytes += COMMAND_HEADER_BYTES + (long) payload.length;
		return this;
	}

	private void requireCommandFits(long payloadBytes) {
		long total = HEADER_BYTES + encodedCommandBytes + COMMAND_HEADER_BYTES + payloadBytes;
		if (total > MAX_PACKET_BYTES) {
			throw new IllegalStateException("packet exceeds the v1 byte limit");
		}
	}

	private static ByteBuffer blitPayload(long source, long destination, Rect sourceRect,
		Rect destinationRect, Rect clip, int transform, int blend) {
		requireSurface(source);
		requireSurface(destination);
		requirePositive(sourceRect, "source rectangle");
		requirePositive(destinationRect, "destination rectangle");
		requireClip(clip);
		if (transform < 0 || transform > 7 ||
			(blend != BLEND_REPLACE && blend != BLEND_SOURCE_OVER)) {
			throw new IllegalArgumentException("transform or blend mode is invalid");
		}
		ByteBuffer payload = littleEndian(72).putLong(source).putLong(destination);
		putRect(payload, sourceRect);
		putRect(payload, destinationRect);
		putRect(payload, clip);
		payload.putInt(transform).putInt(blend);
		return payload;
	}

	private static void validateDescriptor(FrameDescriptor descriptor,
		int pixelBytes, boolean upload) {
		if (descriptor == null || pixelBytes < 0 || pixelBytes > MAX_PIXEL_BYTES ||
			!validScale(descriptor.sourceScale()) || !validScale(descriptor.desiredScale()) ||
			(descriptor.pixelFormat() != PIXEL_FORMAT_RGBA8 &&
				descriptor.pixelFormat() != PIXEL_FORMAT_BGRA8) ||
			(descriptor.alphaMode() < ALPHA_OPAQUE || descriptor.alphaMode() > ALPHA_PREMULTIPLIED) ||
			descriptor.origin() != 1 || (upload && descriptor.frameSequence() != 0)) {
			throw new IllegalArgumentException("frame descriptor fields are invalid");
		}
		if (descriptor.logicalWidth() <= 0 || descriptor.logicalHeight() <= 0 ||
			(long) descriptor.logicalWidth() * descriptor.logicalHeight() > MAX_LOGICAL_PIXELS ||
			(long) descriptor.logicalWidth() * descriptor.sourceScale() != descriptor.physicalWidth() ||
			(long) descriptor.logicalHeight() * descriptor.sourceScale() != descriptor.physicalHeight() ||
			descriptor.physicalStrideBytes() < (long) descriptor.physicalWidth() * 4 ||
			(long) descriptor.physicalStrideBytes() * descriptor.physicalHeight() != pixelBytes) {
			throw new IllegalArgumentException("frame descriptor dimensions or byte count are invalid");
		}
	}

	private static void putDescriptor(ByteBuffer target, FrameDescriptor descriptor) {
		target.putInt(56).putInt(ABI_VERSION).putLong(descriptor.frameSequence())
			.putInt(descriptor.physicalWidth()).putInt(descriptor.physicalHeight())
			.putInt(descriptor.physicalStrideBytes()).putInt(descriptor.logicalWidth())
			.putInt(descriptor.logicalHeight()).putInt(descriptor.sourceScale())
			.putInt(descriptor.desiredScale()).putInt(descriptor.pixelFormat())
			.putInt(descriptor.alphaMode()).putInt(descriptor.origin());
	}

	private static void putRect(ByteBuffer target, Rect rect) {
		target.putInt(rect.x()).putInt(rect.y()).putInt(rect.width()).putInt(rect.height());
	}

	private static ByteBuffer littleEndian(int bytes) {
		return ByteBuffer.allocate(bytes).order(ByteOrder.LITTLE_ENDIAN);
	}

	private static void requireSurface(long surface) {
		// The top bit is reserved for private presentation surfaces created by the native renderer.
		if (surface <= 0) throw new IllegalArgumentException("public surface handle must be in the low half-range");
	}

	private static void requirePositive(Rect rect, String name) {
		if (rect == null || rect.width() <= 0 || rect.height() <= 0) {
			throw new IllegalArgumentException(name + " must have positive dimensions");
		}
	}

	private static void requireClip(Rect rect) {
		if (rect == null || rect.width() < 0 || rect.height() < 0) {
			throw new IllegalArgumentException("clip dimensions cannot be negative");
		}
	}

	private static boolean validScale(int scale) {
		return scale == SCALE_ORIGINAL || scale == SCALE_2X || scale == SCALE_4X;
	}

	public static final class Rect {
		private final int x;
		private final int y;
		private final int width;
		private final int height;

		public Rect(int x, int y, int width, int height) {
			this.x = x;
			this.y = y;
			this.width = width;
			this.height = height;
	}
		public int x() { return x; }
		public int y() { return y; }
		public int width() { return width; }
		public int height() { return height; }
	}

	public static final class FrameDescriptor {
		private final long frameSequence;
		private final int physicalWidth;
		private final int physicalHeight;
		private final int physicalStrideBytes;
		private final int logicalWidth;
		private final int logicalHeight;
		private final int sourceScale;
		private final int desiredScale;
		private final int pixelFormat;
		private final int alphaMode;
		private final int origin;

		public FrameDescriptor(long frameSequence, int physicalWidth, int physicalHeight,
			int physicalStrideBytes, int logicalWidth, int logicalHeight, int sourceScale,
			int desiredScale, int pixelFormat, int alphaMode, int origin) {
			this.frameSequence = frameSequence;
			this.physicalWidth = physicalWidth;
			this.physicalHeight = physicalHeight;
			this.physicalStrideBytes = physicalStrideBytes;
			this.logicalWidth = logicalWidth;
			this.logicalHeight = logicalHeight;
			this.sourceScale = sourceScale;
			this.desiredScale = desiredScale;
			this.pixelFormat = pixelFormat;
			this.alphaMode = alphaMode;
			this.origin = origin;
		}
		public long frameSequence() { return frameSequence; }
		public int physicalWidth() { return physicalWidth; }
		public int physicalHeight() { return physicalHeight; }
		public int physicalStrideBytes() { return physicalStrideBytes; }
		public int logicalWidth() { return logicalWidth; }
		public int logicalHeight() { return logicalHeight; }
		public int sourceScale() { return sourceScale; }
		public int desiredScale() { return desiredScale; }
		public int pixelFormat() { return pixelFormat; }
		public int alphaMode() { return alphaMode; }
		public int origin() { return origin; }
	}

	private static final class Command {
		private final int opcode;
		private final byte[] payload;
		private Command(int opcode, byte[] payload) { this.opcode = opcode; this.payload = payload; }
	}
}
