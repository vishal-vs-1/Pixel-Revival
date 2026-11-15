"""Apply capture and artwork replacement at the MIDP graphics boundary."""

import re

from .anchors import _replace_exactly_once


def patch_drawn_player_poses(text):
    """Flush a buffered arc run before unrelated drawing or a graphics state change."""
    boundary = '// Runtime mutation boundary; nested drawing helpers share one revision.'
    text = text.replace(boundary, boundary + '\n        if (PlayerPoses.ENABLED) flushPendingDrawnPoses();')
    arc = 'public void fillArc(int x, int y, int width, int height, int startAngle, int arcAngle) {'
    old = arc + '\n        ' + boundary + '\n        if (PlayerPoses.ENABLED) flushPendingDrawnPoses();'
    new = arc + '\n        ' + boundary + '''
        if (PlayerPoses.ENABLED && hdRawRgbSuppression == 0) {
            if (contextDisposed) {
                flushPendingDrawnPoses();
                hdMutationOriginal_fillArc(x, y, width, height, startAngle, arcAngle);
                return;
            }
            if (drawnPlayerPoses == null) drawnPlayerPoses = new DrawnPlayerPoses();
            if (baseImage.pendingDrawnGraphics != null && baseImage.pendingDrawnGraphics != this)
                baseImage.pendingDrawnGraphics.flushDrawnPlayerPoses();
            if (drawnPlayerPoses.add(x, y, width, height, startAngle, arcAngle, color)) {
                baseImage.pendingDrawnGraphics = this;
                return;
            }
            flushDrawnPlayerPoses();
            if (drawnPlayerPoses.add(x, y, width, height, startAngle, arcAngle, color)) {
                baseImage.pendingDrawnGraphics = this;
                return;
            }
        }'''
    text = _replace_exactly_once(text, old, new, 'filled arc capture')
    # A flush reads another surface. Complete that surface's run before reading its pixels.
    flush = 'public void flushGraphics(PlatformImage image, int x, int y, int width, int height) {'
    flush += '\n        ' + boundary
    text = _replace_exactly_once(text, flush, flush + '''
        if (PlayerPoses.ENABLED && image != null && image.pendingDrawnGraphics != null)
            image.pendingDrawnGraphics.flushDrawnPlayerPoses();''',
        'drawn sprite flush source')
    for name in ('setClip', 'clipRect', 'translate', 'setOrigin', 'reset', 'getPixel', 'getRGBPixel'):
        text = re.sub(r'(public (?:final )?(?:void|int) ' + name + r'\([^\n]*\)[^\n{]*\s*\{)',
                      r'\1\n        if (PlayerPoses.ENABLED) flushPendingDrawnPoses();', text)
    text = text.replace('// Runtime mutation boundary; RGB presentation uses an immutable source snapshot.',
                        '// Runtime mutation boundary; RGB presentation uses an immutable source snapshot.\n'
                        '        if (PlayerPoses.ENABLED) flushPendingDrawnPoses();')
    helpers = '''
    private DrawnPlayerPoses drawnPlayerPoses;
    private boolean replayingDrawnPoses;
    private void flushPendingDrawnPoses() {
        if (baseImage != null && baseImage.pendingDrawnGraphics != null)
            baseImage.pendingDrawnGraphics.flushDrawnPlayerPoses();
    }
    void flushDrawnPlayerPoses() {
        if (drawnPlayerPoses == null || replayingDrawnPoses) return;
        if (baseImage.pendingDrawnGraphics == this) baseImage.pendingDrawnGraphics = null;
        replayingDrawnPoses = true;
        int previousColor = color;
        try {
            drawnPlayerPoses.flush(arc -> {
                color = arc[6];
                hdMutationOriginal_fillArc(arc[0], arc[1], arc[2], arc[3], arc[4], arc[5]);
            }, (image, x, y) -> hdMutationOriginal_drawImage(image, x, y, 0));
        } finally {
            color = previousColor;
            replayingDrawnPoses = false;
        }
    }
'''
    end = text.rfind('}')
    return text[:end] + helpers + text[end:]


def wrap_public_method(text, signature, mutation_target, suppress_raw=True, post=None):
    """Add one nested-safe mutation boundary around a shared emulator draw API."""
    match = re.search(r'public void ' + signature + r'\s*\{', text)
    if not match:
        return text
    declaration = match[0][:-1].strip()
    name = re.search(r'public void (\w+)', declaration).group(1)
    start = match.start()
    opening = text.index('{', match.start())
    if re.match(r'\s*// Runtime mutation boundary', text[opening + 1:]):
        return text
    depth = 1
    end = opening + 1
    while depth:
        if text[end] == '{': depth += 1
        elif text[end] == '}': depth -= 1
        end += 1
    params = declaration[declaration.index('(') + 1:declaration.rindex(')')]
    args = ', '.join(part.strip().split()[-1] for part in params.split(',') if part.strip())
    original_name = 'hdMutationOriginal_' + name
    original_header = text[start:opening].replace('public void ' + name,
                                                   'private void ' + original_name, 1)
    original = original_header + text[opening:end]
    wrapper = declaration + ''' {
        // Runtime mutation boundary; nested drawing helpers share one revision.
        RuntimeAssets.beginMutation(''' + mutation_target + ''');
        try {
            ''' + ('hdRawRgbSuppression++;\n            try { ' if suppress_raw else '') + original_name + '(' + args + ');' + ('' if post is None else '\n            ' + post) + ('''\n            } finally { hdRawRgbSuppression--; }''' if suppress_raw else '') + '''
        } finally { RuntimeAssets.endMutation(''' + mutation_target + '''); }
    }
'''
    text = text[:start] + original + text[end:]
    return text[:text.rfind('}')] + wrapper + text[text.rfind('}'):]


def patch_platform_graphics(SOURCE):
    graphics = SOURCE / 'mobile/PlatformGraphics.java'
    text = graphics.read_text(encoding='utf-8')
    text=text.replace('hdOriginal_flushGraphics(image, x, y, width, height); return;',
                      'hdOriginal_flushGraphics(image, x, y, width, height); Enhancement.frame(baseImage); return;')
    if 'hdOriginal_fillRect' not in text:
        text=text.replace('public void fillRect(int x, int y, int width, int height)',
                          'private void hdOriginal_fillRect(int x, int y, int width, int height)',1)
        wrapper='''
    public void fillRect(int x,int y,int width,int height) {
        if(Enhancement.SCALE==1 && !GpuRenderer.ENABLED) { hdOriginal_fillRect(x,y,width,height); return; }
        boolean outer=hdSuppress++==0;
        try { hdOriginal_fillRect(x,y,width,height); } finally { hdSuppress--; }
        if(outer) Enhancement.fill(this,x,y,width,height);
    }
'''
        text=text[:text.rfind('}')]+wrapper+'\n}\n'
    if 'hdWrite(' not in text:
        # All software pixel assignments retain their original evaluation order.
        pattern = r'canvasData\[([^\n;]+?)\]\s*=\s*([^;]+);'
        text, count = re.subn(pattern, lambda m: f'hdWrite({m[1]}, {m[2]});', text)
        assert count >= 20, f'Unexpected upstream raster implementation ({count} writes)'
        text = text.replace('protected boolean fastBlit;', '''protected boolean fastBlit;
    private int hdSuppress = 0;
    private void hdWrite(int index, int value) {
        canvasData[index] = value;
        if (hdSuppress == 0) Enhancement.pixel(baseImage, index, value);
    }
    private void hdCopy(int[] source, int offset, int[] destination, int target, int count) {
        System.arraycopy(source, offset, destination, target, count);
        if (hdSuppress == 0) for (int i=0; i<count; i++) Enhancement.pixel(baseImage, target+i, destination[target+i]);
    }
''')
        # The constructor initializes presentation storage before the first write.
        text = text.replace('canvas = image.getCanvas();', 'canvas = image.getCanvas();\n        Enhancement.surface(image);', 1)
        text = re.sub(r'System\.arraycopy\(([^;]+?), canvasData, ([^;]+?)\);', r'hdCopy(\1, canvasData, \2);', text)
        text = text.replace('gc.drawString(str, x, y);', 'gc.drawString(str, x, y);\n            Enhancement.text(this, str, x, y);')

        # Wrappers suppress low-resolution mirror writes while original image drawing runs.
        wrappers = []
        def wrap(signature, call):
            nonlocal text
            match = re.search(r'public void '+signature+r'\s*\{', text)
            assert match, signature
            declaration = match[0][:-1].strip()
            name = re.search(r'public void (\w+)', declaration)[1]
            params = declaration[declaration.index('(')+1:declaration.rindex(')')]
            args = ', '.join(part.strip().split()[-1] for part in params.split(','))
            original = declaration.replace('public void '+name, 'private void hdOriginal_'+name)
            text = text[:match.start()] + text[match.start():].replace(declaration, original, 1)
            wrappers.append(declaration + ''' {
        if (Enhancement.SCALE == 1 && !GpuRenderer.ENABLED) { hdOriginal_'''+name+'('+args+'''); return; }
        boolean outer = hdSuppress++ == 0;
        try { hdOriginal_'''+name+'('+args+'''); }
        finally { hdSuppress--; }
        if (outer) { '''+call+''' }
    }
''')
        wrap(r'drawImage\(Image image, int x, int y, int anchor\)',
             'Enhancement.blit(this, image, 0, 0, image.getWidth(), image.getHeight(), AnchorX(x,image.getWidth(),anchor), AnchorY(y,image.getHeight(),anchor), image.getWidth(), image.getHeight(), 0, false);')
        wrap(r'drawRegion\(Image image, int subx, int suby, int subw, int subh, int transform, int x, int y, int anchor\)',
             '''boolean rotate = transform >= 4;
             int dw = rotate ? subh : subw, dh = rotate ? subw : subh;
             if (Mobile.compatSiemensFriendlyDrawing) { if (translateX < 0) x -= translateX; if (translateY < 0) y -= translateY; }
             Enhancement.blit(this, image, subx, suby, subw, subh, AnchorX(x,dw,anchor), AnchorY(y,dh,anchor), dw, dh, transform, false);''')
        # The core transformed path uses DoJa's enum; convert back to MIDP for the presentation transform.
        wrap(r'drawTransformedImage\(PlatformImage image, int dx, int dy, int width, int height, int sx, int sy, int swidth, int sheight, int transform\)',
             'int[] transforms = {0,2,1,3,6,5,7,4}; Enhancement.blit(this,image,sx,sy,swidth,sheight,dx,dy,width,height,transforms[transform],false);')
        wrap(r'flushGraphics\(PlatformImage image, int x, int y, int width, int height\)',
             '''if (Mobile.maskIndex == 0 && !Mobile.funLightsEnabled) Enhancement.blit(this,image,x,y,width,height,x,y,width,height,0,true);
             else Enhancement.sync(baseImage,x,y,width,height);
             Enhancement.frame(baseImage);''')
        # Persistently drawn geometry uses instrumented low-resolution pixel writes.
        # copyArea must preserve detail, not copy its already reduced logical pixels.
        wrap(r'copyArea\(int x_src, int y_src, int width, int height, int x_dest, int y_dest, int anchor\)',
             '''Enhancement.blit(this,baseImage,x_src+translateX,y_src+translateY,width,height,AnchorX(x_dest,width,anchor),AnchorY(y_dest,height,anchor),width,height,0,true);''')
        text = text[:text.rfind('}')] + '\n' + '\n'.join(wrappers) + '\n}\n'
        graphics.write_text(text, encoding='utf-8')
    graphics.write_text(text, encoding='utf-8')
    text = graphics.read_text(encoding='utf-8')
    if 'hdRawRgbSuppression' not in text:
        text = text.replace('private int hdSuppress = 0;',
                            'private int hdSuppress = 0;\n    private int hdRawRgbSuppression = 0;')
    mutation_apis = [
        r'drawImage\(Image image, int x, int y, int anchor\)',
        r'drawRegion\(Image image, int subx, int suby, int subw, int subh, int transform, int x, int y, int anchor\)',
        r'drawTransformedImage\(PlatformImage image, int dx, int dy, int width, int height, int sx, int sy, int swidth, int sheight, int transform\)',
        r'flushGraphics\(PlatformImage image, int x, int y, int width, int height\)',
        r'copyArea\(int x_src, int y_src, int width, int height, int x_dest, int y_dest, int anchor\)',
        r'fillRect\(int x, int y, int width, int height\)',
        r'clearRect\(int x, int y, int width, int height\)',
        r'drawChar\(char character, int x, int y, int anchor\)',
        r'drawChars\(char\[\] data, int offset, int length, int x, int y, int anchor\)',
        r'drawChars\(char\[\] data, int x, int y, int offset, int length\)',
        r'drawImage\(Image image, int x, int y\)',
        r'drawRegion\(Image image, int subx, int suby, int subw, int subh, int transform, int x, int y, int width_dest, int height_dest, int anchor, int stretch_quality\)',
        r'drawLine\(int x1, int y1, int x2, int y2\)',
        r'drawArc\(int x, int y, int width, int height, int startAngle, int arcAngle\)',
        r'drawRect\(int x, int y, int width, int height\)',
        r'drawRoundRect\(int x, int y, int width, int height, int arcWidth, int arcHeight\)',
        r'drawString\(String str, int x, int y, int anchor\)',
        r'drawSubstring\(String str, int offset, int len, int x, int y, int anchor\)',
        r'fillArc\(int x, int y, int width, int height, int startAngle, int arcAngle\)',
        r'fillRoundRect\(int x, int y, int width, int height, int arcWidth, int arcHeight\)',
        r'drawImage\(javax\.microedition\.lcdui\.Image img, int x, int y, int anchor, int manipulation\)',
        r'drawPixels\(byte\[\] pixels, byte\[\] transparencyMask, int offset, int scanlength, int x, int y, int width, int height, int manipulation, int format\)',
        r'drawPixels\(int\[\] pixels, boolean transparency, int offset, int scanlength, int x, int y, int width, int height, int manipulation, int format\)',
        r'drawPixels\(short\[\] pixels, boolean transparency, int offset, int scanlength, int x, int y, int width, int height, int manipulation, int format\)',
        r'drawPolygon\(int\[\] xPoints, int xOffset, int\[\] yPoints, int yOffset, int nPoints, int argbColor\)',
        r'drawTriangle\(int x1, int y1, int x2, int y2, int x3, int y3\)',
        r'drawTriangle\(int x1, int y1, int x2, int y2, int x3, int y3, int argbColor\)',
        r'fillPolygon\(int\[\] xPoints, int xOffset, int\[\] yPoints, int yOffset, int nPoints, int argbColor\)',
        r'fillTriangle\(int x1, int y1, int x2, int y2, int x3, int y3\)',
        r'fillTriangle\(int x1, int y1, int x2, int y2, int x3, int y3, int argbColor\)',
        r'drawImage\(com\.nttdocomo\.ui\.Image image, int\[\] matrix\)',
        r'drawImage\(com\.nttdocomo\.ui\.Image image, int\[\] matrix, int sx, int sy, int width, int height\)',
        r'drawImage\(com\.nttdocomo\.ui\.Image image, int x, int y\)',
        r'drawImage\(com\.nttdocomo\.ui\.Image image, int dx, int dy, int sx, int sy, int width, int height\)',
        r'drawPolyline\(int\[\] xPoints, int\[\] yPoints, int nPoints\)',
        r'drawPolyline\(int\[\] xPoints, int\[\] yPoints, int offset, int count\)',
        r'fillPolygon\(final int\[\] xPoints, final int\[\] yPoints, final int numPoints\)',
        r'fillPolygon\(final int\[\] xPoints, final int\[\] yPoints, final int offset, final int numPoints\)',
        r'drawPolygon\(final int\[\] xPoints, final int\[\] yPoints, final int numPoints\)',
        r'drawPolygon\(final int\[\] xPoints, final int\[\] yPoints, final int offset, final int numPoints\)',
        r'drawScaledImage\(com\.nttdocomo\.ui\.Image image, int dx, int dy, int width, int height, int sx, int sy, int swidth, int sheight\)',
        r'drawSpriteSet\(com\.nttdocomo\.ui\.SpriteSet sprites\)',
        r'drawImageMap\(com\.nttdocomo\.ui\.ImageMap map, int x, int y\)',
        r'setPixel\(int x, int y\)',
        r'setPixel\(int x, int y, int color\)',
        r'setRGBPixel\(int x, int y, int color\)',
        r'setPixels\(int x, int y, int width, int height, int\[\] array, int offset\)',
        r'setRGBPixels\(int x, int y, int width, int height, int\[\] array, int offset\)',
        r'drawFunLights\(int\[\] pixelData, int width, int height\)',
        r'unlock\(boolean forced\)',
        r'flush\(\)',
        r'drawCommandList\(Texture texture, int x, int y, FigureLayout layout, Effect3D effect, int\[\] commandlist\)',
        r'drawCommandList\(Texture\[\] textures, int x, int y, FigureLayout layout, Effect3D effect, int\[\] commandlist\)',
        r'drawFigure\(Figure figure, int x, int y, FigureLayout layout, Effect3D effect\)',
        r'renderFigure\(Figure figure, int x, int y, FigureLayout layout, Effect3D effect\)',
        r'renderPrimitives\(Texture texture, int x, int y, FigureLayout layout, Effect3D effect, int command, int numPrimitives, int\[\] vertexCoords, int\[\] normals, int\[\] textureCoords, int\[\] colors\)',
        r'drawFigure\(com\.nttdocomo\.opt\.ui\.j3d\.Figure figure\)',
        r'executeCommandList\(int\[\] commandlist\)',
        r'renderFigure\(com\.nttdocomo\.opt\.ui\.j3d\.Figure figure\)',
        r'renderPrimitives\(com\.nttdocomo\.opt\.ui\.j3d\.PrimitiveArray primitives, int command\)',
        r'renderPrimitives\(com\.nttdocomo\.opt\.ui\.j3d\.PrimitiveArray primitives, int offset, int length, int command\)',
    ]
    for signature in mutation_apis:
        text = wrap_public_method(text, signature, 'baseImage')
    text = wrap_public_method(text,
        r'drawRGB\(int\[\] rgbData, int offset, int scanlength, int x, int y, int width, int height, boolean processAlpha\)',
        'baseImage', suppress_raw=False,
        post='if (hdRawRgbSuppression == 0) RuntimeAssets.rawRgbUsed(rgbData, offset, scanlength, width, height, processAlpha);')
    # Upgrade this wrapper on fresh and previously patched checkouts. Logical
    # drawing stays authoritative; suppress only its duplicate GPU pixel writes.
    rgb_wrapper = re.search(r'public void drawRGB\(int\[\] rgbData, int offset, int scanlength, int x, int y, int width, int height, boolean processAlpha\)\s*\{', text)
    if rgb_wrapper is None:
        raise RuntimeError('Shared drawRGB wrapper was not found')
    opening = text.index('{', rgb_wrapper.start())
    end, depth = opening + 1, 1
    while depth:
        if text[end] == '{': depth += 1
        elif text[end] == '}': depth -= 1
        end += 1
    text = text[:opening] + '''{
        // Runtime mutation boundary; RGB presentation uses an immutable source snapshot.
        RuntimeAssets.beginMutation(baseImage);
        try {
            RawRgbPresentation.Draw hdRgb = hdSuppress == 0 && hdRawRgbSuppression == 0
                    ? RawRgbPresentation.prepare(this, rgbData, offset, scanlength, x, y, width, height, processAlpha)
                    : null;
            if (hdRgb != null) hdSuppress++;
            try {
                hdMutationOriginal_drawRGB(rgbData, offset, scanlength, x, y, width, height, processAlpha);
            } finally { if (hdRgb != null) hdSuppress--; }
            if (hdRgb != null) hdRgb.render(this);
            else if (hdRawRgbSuppression == 0)
                RuntimeAssets.rawRgbUsed(rgbData, offset, scanlength, width, height, processAlpha);
        } finally { RuntimeAssets.endMutation(baseImage); }
    }''' + text[end:]
    graphics.write_text(text, encoding='utf-8')
    # Incremental upgrades are kept idempotent for already-patched checkouts.
    text = graphics.read_text(encoding='utf-8')
    # Original GPU play can use the same bulk blits at scale 1.
    text = text.replace('if(Enhancement.SCALE==1) { hdOriginal_fillRect',
                        'if(Enhancement.SCALE==1 && !GpuRenderer.ENABLED) { hdOriginal_fillRect')
    text = text.replace('if (Enhancement.SCALE == 1) { hdOriginal_',
                        'if (Enhancement.SCALE == 1 && !GpuRenderer.ENABLED) { hdOriginal_')
    text = text.replace('PlayerPoses.source(image, 0, 0, image.getWidth(), image.getHeight());',
                        'PlayerPoses.source(image, 0, 0, image.getWidth(), image.getHeight(), x, y);')
    text = text.replace('PlayerPoses.source(image, subx, suby, subw, subh);',
                        'PlayerPoses.source(image, subx, suby, subw, subh, x, y);')
    if 'PlayerPoses.source(' not in text:
        for signature, region, update in (
            ('drawImage(Image image, int x, int y, int anchor)',
             '0, 0, image.getWidth(), image.getHeight()', 'image = replacement;'),
            ('drawRegion(Image image, int subx, int suby, int subw, int subh, int transform, int x, int y, int anchor)',
             'subx, suby, subw, subh', 'image = replacement; subx = 0; suby = 0;'),
        ):
            anchor = 'public void ' + signature + ' {'
            hook = f'''
        if (PlayerPoses.ENABLED && image != null && image != baseImage) {{
            Image replacement = PlayerPoses.source(image, {region}, x, y);
            if (replacement != null) {{ {update} }}
        }}'''
            boundary = anchor + '\n        // Runtime mutation boundary; nested drawing helpers share one revision.'
            text = _replace_exactly_once(text, boundary, boundary + hook, signature)
    if 'void flushDrawnPlayerPoses()' not in text:
        text = patch_drawn_player_poses(text)
    elif 'void flushPendingDrawnPoses()' not in text:
        text = text.replace('if (PlayerPoses.ENABLED) flushDrawnPlayerPoses();',
                            'if (PlayerPoses.ENABLED) flushPendingDrawnPoses();')
        text = text.replace('    void flushDrawnPlayerPoses() {', '''    private void flushPendingDrawnPoses() {
        if (baseImage != null && baseImage.pendingDrawnGraphics != null)
            baseImage.pendingDrawnGraphics.flushDrawnPlayerPoses();
    }
    void flushDrawnPlayerPoses() {''')
    fill = 'public void fillRect(int x,int y,int width,int height) {'
    if fill + '\n        if (PlayerPoses.ENABLED)' not in text and fill in text:
        text = _replace_exactly_once(text, fill, fill + '\n        if (PlayerPoses.ENABLED) flushPendingDrawnPoses();',
                                     'procedural pose fill boundary')
    if 'Enhancement.frame(baseImage)' not in text:
        text = text.replace('else Enhancement.sync(baseImage,x,y,width,height);', 'else Enhancement.sync(baseImage,x,y,width,height); Enhancement.frame(baseImage);')
    # 3D paths preserve original output until a dedicated 3D presentation path exists.
    if '// HD 3D fallback' not in text:
        text = re.sub(r'(mcv3gc\.(?:drawCommandList|drawFigure|renderFigure|renderPrimitives|flush)\([^;]*\);)',
                      r'\1 Enhancement.sync(baseImage,0,0,canvasWidth,canvasHeight); // HD 3D fallback', text)
    graphics.write_text(text, encoding='utf-8')
