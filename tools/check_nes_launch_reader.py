"""Exercise managed NES launch validation and generated-pack staging with Python fixtures."""

from __future__ import annotations

import hashlib
import json
import gc
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from graphics.runtime import GraphicsRuntimeManifest
from systems.nes.rom import NesRomInspector
from systems.nes.runtime import NesSessionFactory
from systems.nes.capture import _HEADER, _TILE
from systems.nes.integration import NesCaptureIntegration
from systems.nes.pack import NesPackGenerator
from runtime_assets.contracts import pixel_recipe
from runtime_assets.store import AssetStore


PROJECT = ROOT / "nes-bridge" / "tests" / "managed" / "ManagedLaunchChecks.csproj"


def _make_generated_pack(workspace: Path, game_id: str) -> tuple[Path, Path]:
    inbox = workspace / ".runtime" / "nes-capture"
    inbox.mkdir(parents=True)
    capture_session = "1" * 32
    palette = bytes(range(192))
    pattern = bytes([0xFF] * 8 + [0] * 8)
    payload = _HEADER.pack(b"RNT1", game_id.encode(), capture_session.encode(), 0, 1, 0, palette)
    payload += _TILE.pack(1, 2, 0x00112233, pattern, 0, 0, 0, 0)
    (inbox / f"{capture_session}-00000000.rnt").write_bytes(payload)

    recipe = pixel_recipe()
    store = AssetStore(workspace)
    integration = NesCaptureIntegration(game_id, recipe)
    integration.admit(
        workspace, store, {"id": game_id}, recipe, True
    )
    pointer = json.loads((workspace / ".runtime" / "nes-pack.json").read_text(encoding="utf-8"))
    bootstrap_manifest = workspace / pointer["manifest"]
    source = store.list_sources()[0]
    output = store.resolve_path(f"textures-cache/{recipe.key}/{source.source_id}.png")
    output.parent.mkdir(parents=True, exist_ok=True)
    Image.new("RGBA", (32, 32), (32, 64, 96, 255)).save(output)
    if not store.mark_imported_ready(source.source_id, recipe.key,
                                     output.relative_to(workspace).as_posix()):
        raise AssertionError("Could not prepare the synthetic ready tile for the pack fixture.")
    generation = NesPackGenerator(workspace).publish(store, recipe, pack_scale=4)
    if generation is None:
        raise AssertionError("Python pack generator did not produce the managed fixture.")
    # The SQLite wrapper's context manager releases connections when collected on Windows.
    gc.collect()
    return generation.manifest, bootstrap_manifest


def _make_pack_variant(source_manifest: Path, workspace: Path, generation: str, *,
                       bad_page_reference: bool = False, wrong_scale: bool = False,
                       missing_page: bool = False) -> Path:
    source_directory = source_manifest.parent
    target_directory = workspace / "nes" / "packs" / generation
    shutil.copytree(source_directory, target_directory)
    document_path = target_directory / "manifest.json"
    document = json.loads(document_path.read_text(encoding="utf-8"))
    document["generation"] = generation
    if wrong_scale:
        document["pack_scale"] = 2
    document_path.write_text(json.dumps(document, sort_keys=True, separators=(",", ":")),
                              encoding="utf-8")
    definition = target_directory / "hires.txt"
    if bad_page_reference:
        text = definition.read_text(encoding="ascii")
        text = text.replace("<tile>0,", "<tile>9,", 1)
        definition.write_text(text, encoding="ascii")
    if missing_page:
        (target_directory / document["pages"][0]).unlink()
    return document_path


def main() -> int:
    dotnet = shutil.which("dotnet") or r"C:\Program Files\dotnet\dotnet.exe"
    if not Path(dotnet).is_file():
        raise RuntimeError("The .NET 10 SDK is required for managed launch reader checks.")
    build_root = ROOT / "build" / "nes-u18b2-managed-checks"
    build_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="fixture-", dir=build_root) as temporary:
        temp = Path(temporary)
        nes_root = temp / "NES_日本"
        source = temp / "source.nes"
        source.write_bytes(b"NES\x1a\x01" + bytes(11) + bytes(16384))
        game = NesRomInspector().inspect(source)
        rom = nes_root / "library" / game.id / "game.nes"
        rom.parent.mkdir(parents=True)
        shutil.copyfile(source, rom)
        workspace = nes_root / "rendered-games" / game.id
        workspace.mkdir(parents=True)

        graphics_root = temp / "graphics"
        graphics_root.mkdir()
        graphics_dll = graphics_root / "revival_graphics.dll"
        graphics_dll.write_bytes(b"synthetic graphics library")
        graphics_manifest_path = graphics_root / "graphics-runtime.json"
        graphics_manifest = GraphicsRuntimeManifest(
            1, "common-v1", 1, hashlib.sha256(graphics_dll.read_bytes()).hexdigest(), graphics_dll
        )
        original_session = NesSessionFactory(session_id_factory=lambda: "a" * 32).create(
            workspace, {"id": game.id}, rom, graphics_manifest,
            original=True, graphics_manifest_path=graphics_manifest_path,
        )
        original_launch = original_session.home.parent / "launch.json"
        if original_session.saves != workspace / "saves" / "mesen":
            raise AssertionError("Mesen saves are not using the stable workspace path.")
        battery = original_session.saves / "battery"
        battery.mkdir()
        (battery / "sample.sav").write_bytes(b"keep across sessions")

        pack_path, bootstrap_pack_path = _make_generated_pack(workspace, game.id)
        bad_reference_pack = _make_pack_variant(
            pack_path, workspace, "d" * 32, bad_page_reference=True
        )
        wrong_scale_pack = _make_pack_variant(
            pack_path, workspace, "e" * 32, wrong_scale=True
        )
        missing_page_pack = _make_pack_variant(
            pack_path, workspace, "f" * 32, missing_page=True
        )

        def make_session(session_id: str, selected_pack: Path | None,
                         *, original: bool = False):
            return NesSessionFactory(session_id_factory=lambda: session_id).create(
                workspace, {"id": game.id}, rom, graphics_manifest,
                original=original, pack_manifest=selected_pack, pack_scale=4,
                graphics_manifest_path=graphics_manifest_path,
            )

        enhanced_session = make_session("b" * 32, pack_path)
        bootstrap_session = make_session("9" * 32, bootstrap_pack_path)
        bad_reference_session = make_session("c" * 32, bad_reference_pack)
        wrong_scale_session = make_session("d" * 32, wrong_scale_pack)
        missing_page_session = make_session("e" * 32, missing_page_pack)
        copy_failure_session = make_session("f" * 32, pack_path)
        next_session = make_session("0" * 32, pack_path)
        if next_session.saves != original_session.saves or (battery / "sample.sav").read_bytes() != b"keep across sessions":
            raise AssertionError("Separate sessions did not reuse persistent NES saves.")

        output = build_root / "out"
        intermediate = build_root / "obj"
        nuget_config = build_root / "NuGet.Config"
        nuget_config.write_text(
            "<configuration><packageSources><clear /></packageSources></configuration>",
            encoding="utf-8",
        )
        dotnet_cli_home = build_root / "dotnet-cli-home"
        environment = os.environ.copy()
        environment["DOTNET_CLI_HOME"] = str(dotnet_cli_home)
        environment["DOTNET_SKIP_FIRST_TIME_EXPERIENCE"] = "1"
        environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1"
        environment["NUGET_PACKAGES"] = str(build_root / "nuget-packages")
        environment["APPDATA"] = str(build_root / "dotnet-appdata")
        restore = subprocess.run(
            [dotnet, "restore", str(PROJECT), f"-p:BaseIntermediateOutputPath={intermediate}{os.sep}",
             "-p:NuGetAudit=false", "--configfile", str(nuget_config)],
            cwd=ROOT, env=environment, capture_output=True, text=True, timeout=180,
        )
        if restore.returncode:
            raise RuntimeError(f"Managed reader restore failed:\n{restore.stdout}\n{restore.stderr}")
        command = [
            dotnet, "run", "--project", str(PROJECT), "--configuration", "Release", "--no-restore",
            f"-p:BaseOutputPath={output}{os.sep}",
            f"-p:BaseIntermediateOutputPath={intermediate}{os.sep}",
            "--", "--self-test", str(original_launch),
        ]
        result = subprocess.run(command, cwd=ROOT, env=environment,
                                capture_output=True, text=True, timeout=180)
        if result.returncode:
            raise RuntimeError(f"Managed reader check failed:\n{result.stdout}\n{result.stderr}")
        print(result.stdout.strip())
        enhanced_command = list(command)
        enhanced_command.insert(enhanced_command.index("--"), "--no-build")
        enhanced_command[-1] = str(enhanced_session.home.parent / "launch.json")
        enhanced_result = subprocess.run(
            enhanced_command, cwd=ROOT, env=environment,
            capture_output=True, text=True, timeout=120,
        )
        if enhanced_result.returncode:
            raise RuntimeError(
                f"Managed Enhanced reader check failed:\n{enhanced_result.stdout}\n{enhanced_result.stderr}"
            )
        print(enhanced_result.stdout.strip())

        bootstrap_command = list(enhanced_command)
        bootstrap_command[-1] = str(bootstrap_session.home.parent / "launch.json")
        bootstrap_result = subprocess.run(
            bootstrap_command, cwd=ROOT, env=environment,
            capture_output=True, text=True, timeout=120,
        )
        if bootstrap_result.returncode:
            raise RuntimeError(
                f"Managed bootstrap pack check failed:\n{bootstrap_result.stdout}\n"
                f"{bootstrap_result.stderr}"
            )
        print("Managed transparent bootstrap pack check passed.")

        pack_command = list(enhanced_command)
        separator = pack_command.index("--")
        pack_command[separator + 1:] = [
            "--pack-self-test",
            str(original_launch),
            str(enhanced_session.home.parent / "launch.json"),
            str(bad_reference_session.home.parent / "launch.json"),
            str(wrong_scale_session.home.parent / "launch.json"),
            str(missing_page_session.home.parent / "launch.json"),
            str(copy_failure_session.home.parent / "launch.json"),
        ]
        pack_result = subprocess.run(
            pack_command, cwd=ROOT, env=environment,
            capture_output=True, text=True, timeout=120,
        )
        if pack_result.returncode:
            raise RuntimeError(
                f"Managed HD pack staging check failed:\n{pack_result.stdout}\n{pack_result.stderr}"
            )
        print(pack_result.stdout.strip())
        print("Python sessions reuse the persistent Mesen save folder.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
