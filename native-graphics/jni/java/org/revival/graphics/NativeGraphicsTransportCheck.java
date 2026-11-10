package org.revival.graphics;

import java.nio.ByteBuffer;
import java.nio.file.Path;
import java.nio.file.Paths;

/** Checks absolute DLL loading, the build handshake and JNI argument validation. */
public final class NativeGraphicsTransportCheck {
	private NativeGraphicsTransportCheck() { }

	public static void main(String[] args) throws Exception {
		if (args.length != 2) throw new IllegalArgumentException("Pass core DLL and JNI DLL paths");
		NativeGraphics graphics = new NativeGraphics(Paths.get(args[0]), Paths.get(args[1]));
		NativeGraphics.BuildInfo info = graphics.buildInfo();
		if (info.featureBits() != 31 || info.buildId().length() != 64) {
			throw new AssertionError("Native build handshake differs from the U07 ABI");
		}
		NativeGraphics.AttachResult unattached = graphics.attach(7, 128L * 1024 * 1024,
			64 * 1024 * 1024, 8192, 2, 2, NativeGraphics.INPUT_MODE_COMMANDS);
		if (unattached.result() != NativeGraphics.Result.NO_CONTEXT || unattached.context() != 0) {
			throw new AssertionError("Attach without a current window context did not fail closed");
		}
		NativeGraphics.Result stale = graphics.submitPacket(1,
			new GraphicsPacketWriter(7, 1).clearFrame().buildDirect());
		if (stale != NativeGraphics.Result.STALE_HANDLE) {
			throw new AssertionError("JNI packet call did not preserve native result code");
		}
		ByteBuffer view = ByteBuffer.allocateDirect(64);
		view.position(8);
		view.limit(40);
		if (graphics.submitPacket(1, view) != NativeGraphics.Result.STALE_HANDLE) {
			throw new AssertionError("A direct packet view did not reach the native ABI");
		}
		if (graphics.pump(1, 0).result() != NativeGraphics.Result.STALE_HANDLE ||
			graphics.present(1, 320, 240, 4, 3, NativeGraphics.FILTER_LINEAR) != NativeGraphics.Result.STALE_HANDLE ||
			graphics.clearFrame(1) != NativeGraphics.Result.STALE_HANDLE ||
			graphics.beginClose(1) != NativeGraphics.Result.STALE_HANDLE ||
			graphics.destroyOnOwner(1) != NativeGraphics.Result.STALE_HANDLE) {
			throw new AssertionError("JNI context lifecycle returned an unexpected native status");
		}
		try {
			graphics.submitPacket(1, ByteBuffer.allocate(40));
			throw new AssertionError("Heap packet buffer was accepted");
		} catch (IllegalArgumentException expected) {
			System.out.println("Absolute JNI load, build handshake and packet transport checks passed.");
		}
	}
}
