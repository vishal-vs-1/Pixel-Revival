"""Protect managed NES controller mappings in settings."""

from .anchors import _require


def patch_nes_config_view_model(source: str) -> str:
    anchor = "\t\tpublic NesInputConfigViewModel Input { get; private set; }"
    _require(source, anchor)
    if "CanEditControllerMappings" not in source:
        source = source.replace(
            anchor,
            anchor + "\n\t\tpublic bool CanEditControllerMappings => global::Mesen.Program.ManagedLaunch == null;",
            1,
        )
    return source


def patch_nes_config_view(source: str) -> str:
    anchor = '<TabItem Header="{l:Translate tpgInput}">'
    managed_anchor = '<TabItem Header="{l:Translate tpgInput}" IsVisible="{Binding CanEditControllerMappings}">'
    if managed_anchor not in source:
        _require(source, anchor)
        source = source.replace(anchor, managed_anchor, 1)
    return source
