"""Patch desktop presentation and control bindings in FreeJ2ME-Plus."""

import re

from .anchors import _replace_exactly_once


CONTROL_PROFILE_PATCH_MARKER = '// JAR Revival control profile input v1'


def patch_control_profile_frontend(text):
    """Install the opt-in AWT adapter without changing the default listener path."""
    if CONTROL_PROFILE_PATCH_MARKER in text:
        if text.startswith(CONTROL_PROFILE_PATCH_MARKER + '\n'):
            text = text[len(CONTROL_PROFILE_PATCH_MARKER) + 1:]
        if CONTROL_PROFILE_PATCH_MARKER not in text:
            text = _replace_exactly_once(text,
                '\tprivate AwtControlProfileInput jmeControlProfileInput;\n',
                '\t' + CONTROL_PROFILE_PATCH_MARKER + '\n\tprivate AwtControlProfileInput jmeControlProfileInput;\n',
                'profile patch marker location')
        if 'import org.recompile.mobile.AwtControlProfileInput;\n' not in text:
            text = _replace_exactly_once(text,
                'import org.recompile.mobile.PlatformGraphics;\n',
                'import org.recompile.mobile.PlatformGraphics;\nimport org.recompile.mobile.AwtControlProfileInput;\n',
                'AWT adapter import')
        return text

    press_body = '''\t\t\tif (MobilePlatform.pressedKeys[mobikey] == false)
\t\t\t{
\t\t\t\tif(mobikey < 20) // Anything over 20 are special keys (fast-forward, etc)
\t\t\t\t{
\t\t\t\t\tMobilePlatform.pressedKeys[mobikey] = true;
\t\t\t\t\tMobilePlatform.keyPressed(Mobile.getMobileKey(mobikey));
\t\t\t\t}
\t\t\t\telse
\t\t\t\t{
\t\t\t\t\tif((e.isAltDown() && e.isControlDown()) || ignoreModifiers)
\t\t\t\t\t{
\t\t\t\t\t\tMobilePlatform.pressedKeys[mobikey] = true;
\t\t\t\t\t}
\t\t\t\t}
\t\t\t}
\t\t\telse
\t\t\t{
\t\t\t\tif(mobikey < 20) { MobilePlatform.keyRepeated(Mobile.getMobileKey(mobikey)); }
\t\t\t}'''
    text = _replace_exactly_once(text, press_body,
        '\t\t\tpressMappedKey(mobikey, ignoreModifiers, e.isAltDown() && e.isControlDown());',
        'default press dispatcher')

    release_body = '''\t\t\t// Figures we must only release if the key is pressed. This vastly simplifies external input event handling
\t\t\tif(MobilePlatform.pressedKeys[mobikey])
\t\t\t{
\t\t\t\tMobilePlatform.pressedKeys[mobikey] = false;
\t\t\t\tMobilePlatform.keyReleased(Mobile.getMobileKey(mobikey));

\t\t\t\tif(mobikey == 21) { ScreenShot.takeScreenshot(false); }
\t\t\t\telse if(mobikey == 22) { MobilePlatform.pauseResumeApp(); }

\t\t\t\tfor(int i = 0; i < MobilePlatform.pressedKeys.length; i++)
\t\t\t\t{
\t\t\t\t\tif(MobilePlatform.pressedKeys[i]) { MobilePlatform.keyRepeated(Mobile.getMobileKey(i)); }
\t\t\t\t}
\t\t\t}'''
    text = _replace_exactly_once(text, release_body,
        '\t\t\treleaseMappedKey(mobikey, true);', 'default release dispatcher')

    text = _replace_exactly_once(text, '\tprivate FJGUI fjGUI;\n',
        '\tprivate FJGUI fjGUI;\n\t' + CONTROL_PROFILE_PATCH_MARKER
        + '\n\tprivate AwtControlProfileInput jmeControlProfileInput;\n',
        'profile adapter field')

    listener = '''\t\t/* Inputs should only register if a jar has been loaded, otherwise the GUI will throw NullPointerException */
\t\tlcd.addKeyListener(new KeyListener()
\t\t{
\t\t\tpublic void keyPressed(KeyEvent e) { pressKey(e, false); }

\t\t\tpublic void keyReleased(KeyEvent e) { releaseKey(e); }

\t\t\tpublic void keyTyped(KeyEvent e) { }

\t\t});'''
    custom_listener = '''\t\t/* Inputs should only register if a jar has been loaded, otherwise the GUI will throw NullPointerException */
\t\tjmeControlProfileInput = AwtControlProfileInput.fromSystemProperty(new AwtControlProfileInput.Dispatcher()
\t\t{
\t\t\tpublic boolean gameLoaded() { return fjGUI.hasLoadedFile(); }
\t\t\tpublic void pressMapped(int mobileIndex) { pressMappedKey(mobileIndex, false, false); }
\t\t\tpublic void releaseMapped(int mobileIndex, boolean repeatRemaining) { releaseMappedKey(mobileIndex, repeatRemaining); }
\t\t\tpublic void legacyPress(KeyEvent event) { pressKey(event, false); }
\t\t\tpublic void legacyRelease(KeyEvent event) { releaseKey(event); }
\t\t});
\t\tlcd.addKeyListener(new KeyListener()
\t\t{
\t\t\tpublic void keyPressed(KeyEvent e)
\t\t\t{
\t\t\t\tif(jmeControlProfileInput.isCustom()) { jmeControlProfileInput.keyPressed(e); }
\t\t\t\telse { pressKey(e, false); }
\t\t\t}
\t\t\tpublic void keyReleased(KeyEvent e)
\t\t\t{
\t\t\t\tif(jmeControlProfileInput.isCustom()) { jmeControlProfileInput.keyReleased(e); }
\t\t\t\telse { releaseKey(e); }
\t\t\t}
\t\t\tpublic void keyTyped(KeyEvent e) { }
\t\t});
\t\tlcd.addFocusListener(new java.awt.event.FocusAdapter()
\t\t{
\t\t\tpublic void focusLost(java.awt.event.FocusEvent e) { jmeControlProfileInput.releaseHeldKeys(); }
\t\t});'''
    text = _replace_exactly_once(text, listener, custom_listener, 'LCD key listener')

    press_anchor = '\tprotected void releaseKey(KeyEvent e)\n'
    press_helper = '''\tprivate void pressMappedKey(int mobikey, boolean ignoreModifiers, boolean modified)
\t{
\t\tif(!fjGUI.hasLoadedFile() || mobikey < 0 || mobikey >= MobilePlatform.pressedKeys.length) { return; }
\t\tif(!MobilePlatform.pressedKeys[mobikey])
\t\t{
\t\t\tif(mobikey < 20)
\t\t\t{
\t\t\t\tMobilePlatform.pressedKeys[mobikey] = true;
\t\t\t\tMobilePlatform.keyPressed(Mobile.getMobileKey(mobikey));
\t\t\t}
\t\t\telse if(modified || ignoreModifiers) { MobilePlatform.pressedKeys[mobikey] = true; }
\t\t}
\t\telse if(mobikey < 20) { MobilePlatform.keyRepeated(Mobile.getMobileKey(mobikey)); }
\t}

'''
    text = _replace_exactly_once(text, press_anchor, press_helper + press_anchor,
                                 'mapped press helper location')

    repeat_anchor = '\tprivate static String getFormattedLocation(String loc)\n'
    release_helper = '''\tprivate void releaseMappedKey(int mobikey, boolean repeatRemaining)
\t{
\t\tif(!fjGUI.hasLoadedFile() || mobikey < 0 || mobikey >= MobilePlatform.pressedKeys.length) { return; }
\t\tif(MobilePlatform.pressedKeys[mobikey])
\t\t{
\t\t\tMobilePlatform.pressedKeys[mobikey] = false;
\t\t\tMobilePlatform.keyReleased(Mobile.getMobileKey(mobikey));
\t\t\tif(mobikey == 21) { ScreenShot.takeScreenshot(false); }
\t\t\telse if(mobikey == 22) { MobilePlatform.pauseResumeApp(); }
\t\t\tif(repeatRemaining)
\t\t\t{
\t\t\t\tfor(int i = 0; i < MobilePlatform.pressedKeys.length; i++)
\t\t\t\t{
\t\t\t\t\tif(MobilePlatform.pressedKeys[i]) { MobilePlatform.keyRepeated(Mobile.getMobileKey(i)); }
\t\t\t\t}
\t\t\t}
\t\t}
\t}

'''
    text = _replace_exactly_once(text, repeat_anchor, release_helper + repeat_anchor,
                                 'mapped release helper location')

    old_window_listeners = '''\t\tmain.addWindowListener(new WindowAdapter()
\t\t{
\t\t\tpublic void windowClosing(WindowEvent e)
\t\t\t{
\t\t\t\tSystem.exit(0);
\t\t\t}
\t\t});

\t\t// Force the main window to pass focus down to the game canvas. Swing
\t\t// seems to need this in order to focus it properly when other Java
\t\t// apps are running alongside FreeJ2ME-Plus.
\t\tmain.addWindowFocusListener(new WindowAdapter()
\t\t{
\t\t\t@Override
\t\t\tpublic void windowGainedFocus(WindowEvent e)
\t\t\t{
\t\t\t\tjavax.swing.SwingUtilities.invokeLater(new Runnable()
\t\t\t\t{
\t\t\t\t\tpublic void run() { lcd.requestFocusInWindow(); }
\t\t\t\t});
\t\t\t}
\t\t});'''
    new_window_listeners = '''\t\tmain.addWindowListener(new WindowAdapter()
\t\t{
\t\t\tpublic void windowClosing(WindowEvent e)
\t\t\t{
\t\t\t\tif(jmeControlProfileInput != null) { jmeControlProfileInput.releaseHeldKeys(); }
\t\t\t\tSystem.exit(0);
\t\t\t}
\t\t});

\t\t// Force the main window to pass focus down to the game canvas. Swing
\t\t// seems to need this in order to focus it properly when other Java
\t\t// apps are running alongside FreeJ2ME-Plus.
\t\tmain.addWindowFocusListener(new WindowAdapter()
\t\t{
\t\t\t@Override
\t\t\tpublic void windowGainedFocus(WindowEvent e)
\t\t\t{
\t\t\t\tjavax.swing.SwingUtilities.invokeLater(new Runnable()
\t\t\t\t{
\t\t\t\t\tpublic void run() { lcd.requestFocusInWindow(); }
\t\t\t\t});
\t\t\t}
\t\t\t@Override
\t\t\tpublic void windowLostFocus(WindowEvent e)
\t\t\t{
\t\t\t\tif(jmeControlProfileInput != null) { jmeControlProfileInput.releaseHeldKeys(); }
\t\t\t}
\t\t});'''
    text = _replace_exactly_once(text, old_window_listeners, new_window_listeners,
                                 'window focus listener')
    text = _replace_exactly_once(text,
        'import org.recompile.mobile.PlatformGraphics;\n',
        'import org.recompile.mobile.PlatformGraphics;\nimport org.recompile.mobile.AwtControlProfileInput;\n',
        'AWT adapter import')
    return text


def patch_frontend(SOURCE):
    frontend = SOURCE / 'freej2me/FreeJ2ME.java'
    text = frontend.read_text(encoding='utf-8')
    text = text.replace('Mobile.getPlatform().getLcdFrontbuffer().getCanvas()', 'org.recompile.mobile.Enhancement.surface(Mobile.getPlatform().getLcdFrontbuffer())')
    text = patch_control_profile_frontend(text)
    frontend.write_text(text, encoding='utf-8')
