/* Testable boundary for copied native packet submission. GPL-3.0-or-later. */
package org.recompile.mobile;

import java.nio.ByteBuffer;
import org.revival.graphics.NativeGraphics;

/** Owns the transport boundary so semantic packet planning can be checked without a window. */
interface GraphicsPacketSink {
    NativeGraphics.Result submit(long context, ByteBuffer packet);
}
