import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import org.recompile.mobile.ComparisonInput;

public final class ComparisonInputCheck {
    public static void main(String[] args) throws Exception {
        List<String> events = new ArrayList<String>();
        CountDownLatch released = new CountDownLatch(1);
        CountDownLatch closed = new CountDownLatch(1);
        ComparisonInput.start((token, pressed) -> {
            synchronized(events) { events.add(token + ":" + pressed); }
            if(!pressed) released.countDown();
        }, () -> closed.countDown());
        if(!released.await(5, TimeUnit.SECONDS)) throw new AssertionError("Key stayed held after EOF");
        if(!closed.await(1, TimeUnit.SECONDS)) throw new AssertionError("Renderer was not asked to close");
        synchronized(events) {
            if(!events.toString().equals("[ARROW_RIGHT:true, ARROW_RIGHT:false]"))
                throw new AssertionError(events.toString());
        }
        System.out.println("comparison input passed");
    }
}
