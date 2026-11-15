"""Patch Java ME image creation and mutation for runtime artwork."""

import re

from .anchors import _replace_exactly_once
from .graphics import wrap_public_method


def normalize_generated_hooks(text):
    """Remove obsolete probes and collapse hooks emitted by older patcher builds."""
    text = re.sub(r'(?m)^[ \t]*DecodedAssetCapture\.(?:capture|resource)\([^;]*;[ \t]*\r?\n', '', text)
    text = re.sub(r'(?m)^[ \t]*Enhancement\.promoteDecoded\(this\);[ \t]*\r?\n', '', text)
    hooks = {
        'RuntimeAssets.created(this, ImageObservation.SourceKind.ENCODED);',
        'RuntimeAssets.created(this, ImageObservation.SourceKind.RGB);',
        'RuntimeAssets.created(this, ImageObservation.SourceKind.RESOURCE);',
        'RuntimeAssets.created(this, ImageObservation.SourceKind.MUTABLE_SNAPSHOT, ImageObservation.SurfaceRole.OFFSCREEN);',
    }
    lines = text.splitlines(True)
    output = []
    previous_hook = None
    for line in lines:
        stripped = line.strip()
        if stripped in hooks:
            if stripped == previous_hook:
                continue
            previous_hook = stripped
            output.append(line)
        else:
            if stripped:
                previous_hook = None
            output.append(line)
    return ''.join(output)


def patch_platform_image(SOURCE):
    image = SOURCE / 'mobile/PlatformImage.java'
    text = normalize_generated_hooks(image.read_text(encoding='utf-8'))
    if 'hdRequested' not in text:
        text=text.replace('public class PlatformImage\n{','public class PlatformImage\n{\n    public volatile boolean hdRequested;\n    public volatile boolean hdCaptureObserved;\n    public volatile long hdCaptureRetryAfter;')
    elif 'hdCaptureObserved' not in text:
        text=text.replace('public volatile boolean hdRequested;','public volatile boolean hdRequested;\n    public volatile boolean hdCaptureObserved;\n    public volatile long hdCaptureRetryAfter;')
    if 'hdSourceId' not in text:
        text=text.replace('public volatile long hdCaptureRetryAfter;','public volatile long hdCaptureRetryAfter;\n    public volatile String hdSourceId;\n    public volatile String hdResolvedRecipe;\n    public volatile boolean hdLookupPending;')
    if 'hdCanvas' not in text:
        text = text.replace('protected BufferedImage canvas;', 'protected BufferedImage canvas;\n\tpublic volatile BufferedImage hdCanvas;')
        # Direct pixel changes must invalidate the corresponding presentation pixel.
        text = text.replace('dataBuffer[y * canvas.getWidth() + x] = color;', 'dataBuffer[y * canvas.getWidth() + x] = color;\n        Enhancement.pixel(this, y * canvas.getWidth() + x, color);')
        image.write_text(text, encoding='utf-8')
    image.write_text(text,encoding='utf-8')
    if 'hdGenerated' not in text:
        text=text.replace('public volatile boolean hdRequested;','public volatile boolean hdRequested;\n    public boolean hdGenerated;')
        def constructor_hook(signature,code):
            nonlocal text
            start=text.index('{',text.index(signature));end=start+1;depth=1
            while depth:
                if text[end]=='{':depth+=1
                elif text[end]=='}':depth-=1
                end+=1
            text=text[:end-1]+code+'\n\t'+text[end-1:]
        constructor_hook('public PlatformImage(int[] rgb, int Width, int Height, boolean processAlpha)',
                         '\n        hdGenerated=true; // Transient, generated pixel data is never an AI discovery candidate.\n')
        constructor_hook('public PlatformImage(Image source)',
                         '\n        Enhancement.derive(this,source,0,0,source.getWidth(),source.getHeight(),0);\n')
        constructor_hook('public PlatformImage(Image image, int x, int y, int Width, int Height, int transform)',
                         '\n        Enhancement.derive(this,image,x,y,Width,Height,transform);\n')
        image.write_text(text,encoding='utf-8')
    text = re.sub(r'(?m)^\s*public boolean hdTransient;\s*\r?\n', '', text)
    text = re.sub(r'(?m)^\s*hdTransient\s*=\s*true;[^\r\n]*\r?\n', '', text)
    def append_constructor_hook(signature, statement):
        nonlocal text
        sig=text.index(signature)
        start=text.index('{',sig)
        end=start+1
        depth=1
        while depth:
            if text[end]=='{': depth+=1
            elif text[end]=='}': depth-=1
            end+=1
        body=text[start:end]
        if statement not in body:
            text=text[:end-1]+'\n        '+statement+'\n\t'+text[end-1:]
    append_constructor_hook('public PlatformImage(String name)',
                            'RuntimeAssets.created(this, ImageObservation.SourceKind.RESOURCE);')
    append_constructor_hook('public PlatformImage(InputStream stream)',
                            'RuntimeAssets.created(this, ImageObservation.SourceKind.RESOURCE);')
    append_constructor_hook('public PlatformImage(byte[] imageData, int imageOffset, int imageLength, boolean mutable)',
                            'RuntimeAssets.created(this, ImageObservation.SourceKind.ENCODED);')
    append_constructor_hook('public PlatformImage(int[] rgb, int Width, int Height, boolean processAlpha)',
                            'RuntimeAssets.created(this, ImageObservation.SourceKind.RGB);')
    append_constructor_hook('public PlatformImage(int Width, int Height)',
                            'RuntimeAssets.created(this, ImageObservation.SourceKind.MUTABLE_SNAPSHOT, ImageObservation.SurfaceRole.OFFSCREEN);')
    append_constructor_hook('public PlatformImage(int Width, int Height, int ARGBcolor)',
                            'RuntimeAssets.created(this, ImageObservation.SourceKind.MUTABLE_SNAPSHOT, ImageObservation.SurfaceRole.OFFSCREEN);')
    # Existing derived-image constructors flow through Enhancement.derive, which
    # classifies them centrally and keeps them out of Phase 3 source capture.
    text = wrap_public_method(text, r'setPixel\(int x, int y, int color\)', 'this', suppress_raw=False)
    if 'PlatformGraphics pendingDrawnGraphics;' not in text:
        anchor = 'public class PlatformImage\n{'
        text = _replace_exactly_once(text, anchor, anchor + '\n    PlatformGraphics pendingDrawnGraphics;',
                                     'procedural pose surface')
    image.write_text(text,encoding='utf-8')
