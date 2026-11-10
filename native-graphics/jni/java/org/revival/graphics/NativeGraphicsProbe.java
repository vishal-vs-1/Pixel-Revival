package org.revival.graphics;

/** Small JNI handshake used to validate the shared graphics feature contract. */
public final class NativeGraphicsProbe {
	static final long FEATURE_SURFACE_COMMANDS = 1L;
	static final long FEATURE_COMPLETED_FRAMES = 1L << 1;
	static final long FEATURE_SCALE2X = 1L << 2;
	static final long FEATURE_ACTUAL_DEVICE = 1L << 3;
	static final long FEATURE_SURFACE_SCALE = 1L << 4;
	static final long EXPECTED_FEATURES = FEATURE_SURFACE_COMMANDS | FEATURE_COMPLETED_FRAMES |
		FEATURE_SCALE2X | FEATURE_ACTUAL_DEVICE | FEATURE_SURFACE_SCALE;

	static {
		// Load the ABI DLL first so Windows can resolve the JNI shim's dependency.
		System.loadLibrary("revival_graphics");
		System.loadLibrary("revival_graphics_jni");
	}

	private NativeGraphicsProbe() { }

	public static native int nativeAbiVersion();
	public static native long nativeFeatureBits();
	public static native void nativeRequireFeature(int feature);
}
