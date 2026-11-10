package org.revival.graphics;

/** Process check: the JNI build handshake advertises implemented features. */
public final class NativeGraphicsProbeCheck {
	private NativeGraphicsProbeCheck() { }

	public static void main(String[] args) {
		if (NativeGraphicsProbe.nativeAbiVersion() != 1) {
			throw new AssertionError("Unexpected graphics ABI version");
		}
		if (NativeGraphicsProbe.nativeFeatureBits() != NativeGraphicsProbe.EXPECTED_FEATURES) {
			throw new AssertionError("The native graphics feature bits do not match U07");
		}
		NativeGraphicsProbe.nativeRequireFeature(
			(int) NativeGraphicsProbe.FEATURE_COMPLETED_FRAMES);
		NativeGraphicsProbe.nativeRequireFeature(
			(int) NativeGraphicsProbe.FEATURE_SURFACE_COMMANDS);
		NativeGraphicsProbe.nativeRequireFeature(
			(int) NativeGraphicsProbe.FEATURE_SURFACE_SCALE);
		try {
			NativeGraphicsProbe.nativeRequireFeature(1 << 16);
			throw new AssertionError("Unknown graphics feature was accepted");
		} catch (IllegalArgumentException expected) {
			System.out.println("ABI 1; ordered commands/frame/Scale2x/device/surface-scale features 31.");
		}
	}
}
