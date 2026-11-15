"""Verify pinned source anchors before applying patches."""




def _require(source: str, anchor: str) -> None:
    if anchor not in source:
        raise ValueError(f"Mesen bridge anchor missing: {anchor}")
