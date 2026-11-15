"""Wrap MIDP 3D calls with an explicit presentation boundary."""

import re


def patch_m3g(path):
    """Keep raw software 3D writes inside one nested-safe presentation boundary."""
    text=path.read_text(encoding='utf-8')
    signatures=[r'clear\(Background background\)',r'render\(World world\)',
                r'render\(Node node, Transform transform\)',
                r'render\(VertexBuffer vertices, IndexBuffer triangles, Appearance appearance, Transform transform\)',
                r'render\(VertexBuffer vertices, IndexBuffer triangles, Appearance appearance, Transform transform, int scope\)']
    for signature in signatures:
        match=re.search(r'public void '+signature+r'\s*\{',text)
        if match is None:raise RuntimeError('M3G entry point changed: '+signature)
        opening=text.index('{',match.start())
        if '// M3G presentation boundary' in text[opening:opening+100]:continue
        end=opening+1;depth=1
        while depth:
            if text[end]=='{':depth+=1
            elif text[end]=='}':depth-=1
            end+=1
        declaration=text[match.start():opening].strip()
        name=re.search(r'void (\w+)',declaration).group(1)
        params=declaration[declaration.index('(')+1:declaration.rindex(')')]
        args=', '.join(part.strip().split()[-1] for part in params.split(','))
        original=text[match.start():end].replace('public void '+name,'private void m3gOriginal_'+name,1)
        wrapper=declaration+''' {
        // M3G presentation boundary; nested render calls publish once.
        try (org.recompile.mobile.M3gPresentation.Pass pass = org.recompile.mobile.M3gPresentation.begin(
                target, originX + viewx + viewClipL, originY + viewy + viewClipT,
                viewClipR - viewClipL, viewClipB - viewClipT)) {
            m3gOriginal_'''+name+'('+args+''');
        }
    }
'''
        text=text[:match.start()]+original+text[end:]
        text=text[:text.rfind('}')]+wrapper+text[text.rfind('}'):]
    path.write_text(text,encoding='utf-8')
