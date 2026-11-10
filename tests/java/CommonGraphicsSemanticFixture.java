package org.recompile.mobile;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import org.revival.graphics.NativeGraphics;

/** CPU-only packet intent check for the guarded common Java semantic adapter. */
public final class CommonGraphicsSemanticFixture {
    private static final class FakeSink implements GraphicsPacketSink {
        final List<byte[]> packets = new ArrayList<byte[]>();
        @Override public NativeGraphics.Result submit(long context, ByteBuffer packet) {
            ByteBuffer copy = ByteBuffer.allocate(packet.remaining());
            copy.put(packet.duplicate()).flip();
            byte[] bytes = new byte[copy.remaining()];
            copy.get(bytes);
            packets.add(bytes);
            return NativeGraphics.Result.OK;
        }
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static int opcodeCount(List<byte[]> packets, int opcode) {
        int count = 0;
        for (byte[] packet : packets) {
            ByteBuffer input = ByteBuffer.wrap(packet).order(ByteOrder.LITTLE_ENDIAN);
            int commands = input.getInt(12);
            int offset = 32;
            for (int index = 0; index < commands; index++) {
                if ((input.getShort(offset) & 0xffff) == opcode) count++;
                offset += input.getInt(offset + 4);
            }
        }
        return count;
    }

    public static void main(String[] args) throws InterruptedException {
        FakeSink sink = new FakeSink();
        CommonGraphicsBackend backend = new CommonGraphicsBackend(sink, 7, 9, 176, 220, 4, "fixture");
        PlatformImage source = new PlatformImage(4, 4, 0xff204060);
        PlatformImage target = new PlatformImage(4, 4, 0x00000000);
        backend.observe(source);
        backend.observe(target);
        int afterObserve = sink.packets.size();
        check(afterObserve >= 6, "surface creation and upload packets were not emitted");
        backend.pixel(source, 0, 0x80ff0000);
        backend.derive(target, source, 0, 0, 2, 2, 3);
        backend.publish(target);
        check(opcodeCount(sink.packets, 1) == 4, "each image must create one source and destination pair");
        check(opcodeCount(sink.packets, 11) >= 2, "source surfaces must use the shared scaling command");
        check(opcodeCount(sink.packets, 5) >= 1, "mutable pixel updates must use a bounded fill");
        check(opcodeCount(sink.packets, 6) >= 1, "derived blit semantics were not encoded");
        check(opcodeCount(sink.packets, 9) == 1, "exactly one committed frame was expected");
        backend.publish(target);
        CountDownLatch started = new CountDownLatch(1);
        CountDownLatch finished = new CountDownLatch(1);
        Thread publisher = new Thread(() -> {
            started.countDown();
            backend.publish(target);
            finished.countDown();
        });
        publisher.setDaemon(true);
        publisher.start();
        check(started.await(1, TimeUnit.SECONDS), "publisher did not start");
        check(!finished.await(100, TimeUnit.MILLISECONDS), "third commit must wait for the native owner");
        backend.completeFrames(1);
        check(finished.await(1, TimeUnit.SECONDS), "owner completion must release exactly one frame slot");
        check(opcodeCount(sink.packets, 9) == 3, "the waiting commit must submit once");
        backend.stopFramePublication();
        backend.publish(target);
        check(opcodeCount(sink.packets, 9) == 3, "stopping frame publication must reject a later commit");
        backend.shutdown();
        System.out.println("Common graphics semantic fixture passed");
    }
}
