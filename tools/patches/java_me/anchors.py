"""Find pinned Java patch anchors and reject ambiguous matches."""




def _replace_exactly_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f'Control profile patch expected one {label} anchor; found {count}.')
    return text.replace(old, new, 1)
