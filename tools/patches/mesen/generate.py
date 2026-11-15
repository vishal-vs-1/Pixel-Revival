"""Build an isolated patched Mesen source tree."""

from __future__ import annotations

import json
from pathlib import Path

from .anchors import _require
from .core import patch_factory, patch_nes_console_header, patch_nes_console_cpp, patch_emu_api
from .projects import patch_project, patch_core_project
from .video import patch_hd_nes_ppu_header, patch_hd_nes_ppu_cpp, patch_video_renderer_header, patch_video_renderer_cpp, patch_stock_windows_renderer
from .startup import patch_program, patch_config_manager, patch_dependency_helper, patch_load_rom_helper
from .controls import patch_nes_config_view_model, patch_nes_config_view
from .window import patch_main_window


ROOT = Path(__file__).resolve().parents[3]
UPSTREAM = ROOT / "nes-bridge" / "mesen" / "upstream.json"


def generate(source_root: Path, output_root: Path, *, inprocess: bool = False) -> None:
    metadata = json.loads(UPSTREAM.read_text(encoding="utf-8"))
    factory_path = source_root / metadata["factory_source"]
    project_path = source_root / metadata["interop_project"]
    core_project_path = source_root / metadata["core_project"]
    video_header_path = source_root / metadata["video_renderer_header"]
    video_cpp_path = source_root / metadata["video_renderer_source"]
    stock_renderer_path = source_root / metadata["stock_windows_renderer_source"]
    hd_ppu_header_path = source_root / metadata["hd_nes_ppu_header"]
    hd_ppu_cpp_path = source_root / metadata["hd_nes_ppu_source"]
    nes_console_header_path = source_root / metadata["nes_console_header"]
    nes_console_cpp_path = source_root / metadata["nes_console_source"]
    nes_config_view_model_path = source_root / metadata["managed_nes_config_view_model"]
    nes_config_view_path = source_root / metadata["managed_nes_config_view"]
    emu_api_path = source_root / metadata["managed_emu_api"]
    main_window_path = source_root / metadata["managed_main_window"]
    program_path = source_root / metadata["managed_program"]
    config_manager_path = source_root / metadata["managed_config_manager"]
    dependency_helper_path = source_root / metadata["managed_dependency_helper"]
    load_rom_helper_path = source_root / metadata["managed_load_rom_helper"]
    factory = factory_path.read_text(encoding="utf-8")
    project = project_path.read_text(encoding="utf-8")
    core_project = core_project_path.read_text(encoding="utf-8")
    video_header = video_header_path.read_text(encoding="utf-8")
    video_cpp = video_cpp_path.read_text(encoding="utf-8")
    hd_ppu_header = hd_ppu_header_path.read_text(encoding="utf-8")
    hd_ppu_cpp = hd_ppu_cpp_path.read_text(encoding="utf-8")
    nes_console_header = nes_console_header_path.read_text(encoding="utf-8")
    nes_console_cpp = nes_console_cpp_path.read_text(encoding="utf-8")
    nes_config_view_model = nes_config_view_model_path.read_text(encoding="utf-8")
    nes_config_view = nes_config_view_path.read_text(encoding="utf-8")
    emu_api = emu_api_path.read_text(encoding="utf-8")
    main_window = main_window_path.read_text(encoding="utf-8")
    program = program_path.read_text(encoding="utf-8")
    config_manager = config_manager_path.read_text(encoding="utf-8")
    dependency_helper = dependency_helper_path.read_text(encoding="utf-8")
    load_rom_helper = load_rom_helper_path.read_text(encoding="utf-8")
    for anchor in metadata["anchors"]:
        _require(factory if anchor in factory else project, anchor)
    patched_factory = patch_factory(factory, inprocess=inprocess)
    patched_project = patch_project(project)
    patched_core_project = patch_core_project(core_project)
    patched_video_header = patch_video_renderer_header(video_header)
    patched_video_cpp = patch_video_renderer_cpp(video_cpp)
    patched_hd_ppu_header = patch_hd_nes_ppu_header(hd_ppu_header)
    patched_hd_ppu_cpp = patch_hd_nes_ppu_cpp(hd_ppu_cpp)
    patched_nes_console_header = patch_nes_console_header(nes_console_header)
    patched_nes_console_cpp = patch_nes_console_cpp(nes_console_cpp)
    patched_nes_config_view_model = patch_nes_config_view_model(nes_config_view_model)
    patched_nes_config_view = patch_nes_config_view(nes_config_view)
    patched_emu_api = patch_emu_api(emu_api)
    patched_main_window = patch_main_window(main_window)
    patched_program = patch_program(program)
    patched_config_manager = patch_config_manager(config_manager)
    patched_dependency_helper = patch_dependency_helper(dependency_helper)
    patched_load_rom_helper = patch_load_rom_helper(load_rom_helper)
    (output_root / metadata["factory_source"]).parent.mkdir(parents=True, exist_ok=True)
    (output_root / metadata["interop_project"]).parent.mkdir(parents=True, exist_ok=True)
    (output_root / metadata["factory_source"]).write_text(patched_factory, encoding="utf-8")
    (output_root / metadata["interop_project"]).write_text(patched_project, encoding="utf-8")
    (output_root / metadata["core_project"]).parent.mkdir(parents=True, exist_ok=True)
    (output_root / metadata["core_project"]).write_text(patched_core_project, encoding="utf-8")
    for relative, content in (
        (metadata["video_renderer_header"], patched_video_header),
        (metadata["video_renderer_source"], patched_video_cpp),
        (metadata["stock_windows_renderer_source"],
         patch_stock_windows_renderer(stock_renderer_path.read_text(encoding="utf-8"))),
        (metadata["hd_nes_ppu_header"], patched_hd_ppu_header),
        (metadata["hd_nes_ppu_source"], patched_hd_ppu_cpp),
        (metadata["nes_console_header"], patched_nes_console_header),
        (metadata["nes_console_source"], patched_nes_console_cpp),
        (metadata["managed_emu_api"], patched_emu_api),
        (metadata["managed_main_window"], patched_main_window),
        (metadata["managed_program"], patched_program),
        (metadata["managed_config_manager"], patched_config_manager),
        (metadata["managed_dependency_helper"], patched_dependency_helper),
        (metadata["managed_load_rom_helper"], patched_load_rom_helper),
        (metadata["managed_nes_config_view_model"], patched_nes_config_view_model),
        (metadata["managed_nes_config_view"], patched_nes_config_view),
    ):
        target = output_root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")

    for name in ("managed_launch_reader", "managed_launch_startup", "managed_nes_controls",
                 "managed_hd_pack_validator", "managed_hd_pack_stager", "managed_hd_playback_status"):
        relative = metadata[name]
        target = output_root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        source = ROOT / "nes-bridge" / "managed" / Path(relative).name
        target.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")
