package org.revival.graphics;

import java.nio.ByteBuffer;

/** CPU-only byte and bounds fixtures for the Java v1 packet writer. */
public final class GraphicsPacketWriterCheck {
	private GraphicsPacketWriterCheck() { }

	public static void main(String[] args) {
		byte[] expectedClear = fromHex(
			"52564731010020002800000001000000070000000000000009000000000000000a00000008000000");
		ByteBuffer packet = new GraphicsPacketWriter(7, 9).clearFrame().buildDirect();
		if (!packet.isDirect() || packet.remaining() != expectedClear.length) {
			throw new AssertionError("Clear-frame packet storage or size changed");
		}
		for (byte expected : expectedClear) {
			if (packet.get() != expected) throw new AssertionError("Clear-frame wire bytes differ");
		}

		GraphicsPacketWriter.FrameDescriptor descriptor = new GraphicsPacketWriter.FrameDescriptor(
			0, 2, 1, 8, 2, 1, 1, 2,
			GraphicsPacketWriter.PIXEL_FORMAT_RGBA8,
			GraphicsPacketWriter.ALPHA_STRAIGHT, 1);
		byte[] row = new byte[8];
		GraphicsPacketWriter upload = new GraphicsPacketWriter(4, 1)
			.createSurface(1, 2, 1, 1)
			.beginUpload(1, descriptor, 8)
			.uploadRows(1, 0, 1, 8, row)
			.endUpload(1);
		row[0] = 99;
		ByteBuffer encodedUpload = upload.buildDirect();
		if (encodedUpload.remaining() != 32 + (8 + 20) + (8 + 68) + (8 + 32) + (8 + 8)) {
			throw new AssertionError("Lifecycle packet command order or lengths differ");
		}
		if (encodedUpload.get(168) != 0) {
			throw new AssertionError("The writer retained a caller-owned row buffer");
		}
		ByteBuffer scaled = new GraphicsPacketWriter(4, 2).scaleSurface(1, 2,
			GraphicsPacketWriter.SCALE_2X).buildDirect();
		if (scaled.remaining() != 64 || scaled.getShort(32) != 11 || scaled.getInt(36) != 32) {
			throw new AssertionError("Surface-scale command bytes differ");
		}
		try {
			new GraphicsPacketWriter(4, 2).beginUpload(1,
				new GraphicsPacketWriter.FrameDescriptor(0, 3, 1, 12, 2, 1, 1, 2,
					GraphicsPacketWriter.PIXEL_FORMAT_RGBA8,
					GraphicsPacketWriter.ALPHA_STRAIGHT, 1), 12);
			throw new AssertionError("Inconsistent frame dimensions were accepted");
		} catch (IllegalArgumentException expected) {
		}
		try {
			new GraphicsPacketWriter(0, 2);
			throw new AssertionError("Zero session epoch was accepted");
		} catch (IllegalArgumentException expected) {
		}
		try {
			new GraphicsPacketWriter(4, 3).createSurface(Long.MIN_VALUE, 2, 1, 1);
			throw new AssertionError("Reserved internal surface namespace was accepted");
		} catch (IllegalArgumentException expected) {
			try {
				new GraphicsPacketWriter(4, 3).scaleSurface(1, 2, GraphicsPacketWriter.SCALE_ORIGINAL);
				throw new AssertionError("Original-density surface-scale command was accepted");
			} catch (IllegalArgumentException scaleExpected) {
			System.out.println("Java packet writer clear/lifecycle/bounds fixtures passed.");
			}
		}
	}

	private static byte[] fromHex(String value) {
		byte[] result = new byte[value.length() / 2];
		for (int index = 0; index < result.length; index++) {
			result[index] = (byte) Integer.parseInt(value.substring(index * 2, index * 2 + 2), 16);
		}
		return result;
	}
}
