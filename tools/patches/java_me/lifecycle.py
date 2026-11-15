"""Patch platform input, screen roles, and MIDP event lifecycle behavior."""

import re


def patch_platform_lifecycle(SOURCE):
    loader=SOURCE/'mobile/MIDletLoader.java'
    text=re.sub(r'(?m)^[ \t]*DecodedAssetCapture\.resource\([^;]*;[ \t]*\r?\n', '',
                loader.read_text(encoding='utf-8'))
    loader.write_text(text,encoding='utf-8')
    platform=SOURCE/'mobile/MobilePlatform.java'
    text=platform.read_text(encoding='utf-8')
    if '// Explicit emulator-owned screen role' not in text:
        text=text.replace('lcdFrontbuffer = new PlatformImage(width, height);',
            'lcdFrontbuffer = new PlatformImage(width, height);\n        RuntimeAssets.markScreen(lcdFrontbuffer); // Explicit emulator-owned screen role')
        text=text.replace('lcd = new PlatformImage(width, height);',
            'lcd = new PlatformImage(width, height);\n        RuntimeAssets.markScreen(lcd);')
        platform.write_text(text,encoding='utf-8')
    text=platform.read_text(encoding='utf-8')
    if '// Input queue telemetry' not in text:
        begin=text.index('public static void keyPressed(final int keycode)')
        end=text.index('public static void keyReleased',begin)
        block=text[begin:end]
        block=block.replace('Mobile.getDisplay().postInputEvent(new Runnable()',
                            'final long queuedAt=System.nanoTime(); // Input queue telemetry\n                Mobile.getDisplay().postInputEvent(new Runnable()',1)
        block=block.replace('if(!handleCommands(', 'Performance.input(queuedAt);\n                        if(!handleCommands(',1)
        text=text[:begin]+block+text[end:]
        platform.write_text(text,encoding='utf-8')
    text=platform.read_text(encoding='utf-8')
    if '// Bound keyboard target' not in text:
        for name,next_name in [('keyPressed','keyReleased'),('keyReleased','keyRepeated'),('keyRepeated','pointerDragged')]:
            begin=text.index('public static void '+name+'(');end=text.index('public static void '+next_name+'(',begin)
            block=text[begin:end]
            opening=block.index('{')+1
            block=block[:opening]+'\n        // Bound keyboard target: later UI events cannot retarget a queued key.\n        final Displayable keyTarget=Mobile.getDisplay()==null?null:Mobile.getDisplay().getCurrent();\n'+block[opening:]
            block=block.replace('(displayable = Mobile.getDisplay().getCurrent()) != null','keyTarget != null')
            block=re.sub(r'\bdisplayable\b','keyTarget',block)
            block=block.replace('handleCommands(Mobile.getCanvasAction(keycode))','handleCommands(keyTarget,Mobile.getCanvasAction(keycode))')
            text=text[:begin]+block+text[end:]
        begin=text.index('private static boolean handleCommands(int key)');end=text.index('public boolean load(',begin)
        block=text[begin:end].replace('handleCommands(int key)','handleCommands(Displayable keyTarget,int key)')
        block=re.sub(r'\bdisplayable\b','keyTarget',block)
        text=text[:begin]+block+text[end:];platform.write_text(text,encoding='utf-8')
    timing=SOURCE/'mobile/MIDletEnhancements.java'
    text=timing.read_text(encoding='utf-8')
    if '// Preserve game yield calibration on the GPU path.' not in text:
        text=text.replace('Mobile.getPlatform().limitFps();',
            '// Preserve game yield calibration on the GPU path.\n        if(Boolean.getBoolean("jme.gpu")) {Thread.yield();return;}\n        Mobile.getPlatform().limitFps();')
        timing.write_text(text,encoding='utf-8')
    mobile=SOURCE/'mobile/Mobile.java'
    text=mobile.read_text(encoding='utf-8')
    text=text.replace('String repaintOnSetCur = config.settings.get("compatrepaintonsetcurrent");',
        'String repaintOnSetCur = System.getProperty("jme.compat.repaintOnShow",config.settings.get("compatrepaintonsetcurrent"));')
    mobile.write_text(text,encoding='utf-8')


def patch_midp_lifecycle(SOURCE):
    # MIDP 2.0's LAYOUT_2 flag is valid (MrBeanRacer's wrapper uses 0x4001).
    item = SOURCE.parents[1] / 'javax/microedition/lcdui/Item.java'
    text = item.read_text(encoding='utf-8')
    text = text.replace('LAYOUT_NEWLINE_BEFORE | LAYOUT_NEWLINE_AFTER;',
                        'LAYOUT_NEWLINE_BEFORE | LAYOUT_NEWLINE_AFTER | LAYOUT_2;')
    item.write_text(text, encoding='utf-8')
    display = SOURCE.parents[1] / 'javax/microedition/lcdui/Display.java'
    text = display.read_text(encoding='utf-8')
    if 'runEventSafely' not in text:
        for name in ['inputTask', 'pendingPaint', 'pendingSerial', 'call']:
            text = text.replace(name + '.run();', 'runEventSafely(' + name + ');')
        helper = '''
    private static void runEventSafely(Runnable task) {
        try { task.run(); }
        catch (RuntimeException error) {
            Mobile.log(Mobile.LOG_ERROR, "MIDlet event failed; event processing will continue: " + error);
            error.printStackTrace();
        }
    }
'''
        text = text[:text.rfind('}')] + helper + '\n}\n'
    display.write_text(text, encoding='utf-8')
    canvas=SOURCE.parents[1]/'javax/microedition/lcdui/Canvas.java'
    text=canvas.read_text(encoding='utf-8')
    if 'paintInProgress' not in text:
        text=text.replace('protected volatile boolean needsRepaint = false;',
                          'protected volatile boolean needsRepaint = false;\n    protected volatile boolean paintInProgress = false;\n    protected volatile Thread paintThread;')
        begin=text.index('\tpublic void repaintRequest()')
        end=text.index('\tpublic void setFullScreenMode',begin)
        replacement='''
    public void repaintRequest() {
        if(!isShown() || listCommands) return;
        int renderX,renderY,renderW,renderH;
        synchronized(paintLock) {
            if(!needsRepaint || paintInProgress) {paintLock.notifyAll();return;}
            renderX=paintX;renderY=paintY;renderW=paintW;renderH=paintH;
            needsRepaint=false;
            paintInProgress=true;paintThread=Thread.currentThread();
        }
        try {
            try {
                graphics.reset(renderX,renderY,renderW,renderH);
                paint(graphics);
            } catch(Exception error) {
                Mobile.log(Mobile.LOG_ERROR,"Exception in Canvas paint: "+error);error.printStackTrace();
            }
            Mobile.getPlatform().setPostFlushDraw(postFlushDraw);
            if(hasFlushedToScreen) {hasFlushedToScreen=false;return;}
            Mobile.getPlatform().flushGraphics(platformImage,renderX,renderY,renderW,renderH);
            hasFlushedToScreen=false;
        } finally {
            synchronized(paintLock) {
                paintThread=null;paintInProgress=false;paintLock.notifyAll();
            }
        }
    }

    public void serviceRepaints() {
        while(Mobile.isPaused) {
            try {Thread.sleep(50);} catch(InterruptedException error){Thread.currentThread().interrupt();return;}
        }
        if(!isShown() || listCommands || Thread.currentThread()==paintThread)return;
        if(Mobile.getDisplay().isEventThread()) {repaintRequest();return;}
        for(;;) {
            boolean rescue;
            synchronized(paintLock) {
                if(!isShown() || (!needsRepaint&&!paintInProgress))return;
                try {paintLock.wait(1000);} catch(InterruptedException error){Thread.currentThread().interrupt();return;}
                rescue=needsRepaint&&!paintInProgress;
            }
            if(rescue&&isShown())repaintRequest();
        }
    }

'''
        text=text[:begin]+replacement+text[end:]
        canvas.write_text(text,encoding='utf-8')
