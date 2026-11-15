"""Capture, review and replace a moving NES sprite pose in an isolated workspace."""

import argparse
from hashlib import sha256
import json
import os
from pathlib import Path
import shutil
import sys
from tempfile import mkdtemp
from uuid import uuid4

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from storage.atomic import atomic_json
from runtime_assets.contracts import pixel_recipe
from runtime_assets.pixel import enhance
from systems.nes.capture import owned_path
from systems.nes.pack import NesPackGenerator
from systems.nes.rom import NesRomInspector
from systems.nes.runtime import NesManagedRuntime, NesProcessRuntime
from systems.nes.sprite_lab import moving_poses, replacement_atlas

BUILD = ROOT / "build"
RUNTIME = BUILD / "common-runtime-auto-groups-repair" / "nes"
FEATURE = "nes-auto-sprite-groups-v1"
SCALE = 4


def _runtime():
    info = json.loads((RUNTIME / "build-info.json").read_bytes())
    runtime = NesManagedRuntime(RUNTIME / "player/Mesen.exe", RUNTIME / "graphics-runtime.json")
    if (FEATURE not in info.get("features", ()) or not runtime.available()
            or sha256((runtime.executable.parent / "MesenCore.dll").read_bytes()).hexdigest()
            != info.get("native_core_sha256")):
        raise RuntimeError("The isolated sprite-group Mesen runtime is missing or has changed")
    return runtime


def _paths(run):
    if run.is_symlink() or run.is_junction():
        raise ValueError("The sprite lab run folder is redirected")
    run = run.resolve(strict=True)
    if run.parent != BUILD.resolve() or not run.name.startswith("nes-sprite-lab-"):
        raise ValueError("Choose a run folder created by the sprite lab under build/")
    info = json.loads(owned_path(run, "run.json").read_bytes())
    game_id = info.get("game_id")
    if not isinstance(game_id, str) or len(game_id) != 64 or any(c not in "0123456789abcdef" for c in game_id):
        raise ValueError("Invalid lab cartridge ID")
    source = owned_path(run, f"nes/library/{game_id}/game.nes")
    workspace = owned_path(run, f"nes/rendered-games/{game_id}")
    if NesRomInspector().inspect(source).id != game_id:
        raise ValueError("The isolated cartridge does not match its recorded identity")
    return run, info, source, workspace


def _create_session(runtime, game_id, source, workspace, *, generation=None):
    graphics = runtime._graphics()
    pack = NesPackGenerator(workspace).ensure_capture_pack(pixel_recipe(), pack_scale=SCALE)
    session = runtime.session_factory.create(
        workspace, {"id": game_id}, source, graphics, original=False,
        pack_manifest=pack.manifest, pack_scale=SCALE, graphics_manifest_path=runtime.graphics_manifest,
    )
    if generation:
        # Only this explicitly selected build reads the optional schema-2 overlay.
        atomic_json(session.home.parent / "launch.json",
                    {**session.document(), "schema": 2, "auto_group_generation": generation})
    return session, graphics


def _launch(runtime, game_id, source, workspace, *, capture=False, generation=None):
    session, graphics = _create_session(runtime, game_id, source, workspace, generation=generation)
    environment = os.environ.copy()
    environment["J2ME_REVIVAL_ENABLE_NES_ENHANCED"] = "1"
    environment["J2ME_REVIVAL_NES_CAPTURE"] = "1" if capture else "0"
    environment["J2ME_REVIVAL_NES_LAYOUTS"] = "1" if capture else "0"
    environment["PATH"] = str(graphics.dll.parent) + os.pathsep + environment.get("PATH", "")
    return session, *NesProcessRuntime(runtime.executable).launch(session, environment=environment)


def _review(run, info, workspace):
    session = info["capture_session"]
    poses = moving_poses(workspace, info["game_id"], session)
    preview = owned_path(run, "poses")
    preview.mkdir(exist_ok=True)
    for pose in poses:
        pose.image().resize((128, 128), Image.Resampling.NEAREST).save(owned_path(preview, f"{pose.key}.png"))
        print(f"{pose.key}  {len(pose.positions)} samples  {pose.positions[0][1:]} -> {pose.positions[-1][1:]}")
    print(f"{len(poses)} moving pose candidates. Previews: {preview}")
    return poses


def _stage_overlay(workspace, game_id, pose, artwork):
    page, cells = replacement_atlas(pose, artwork, SCALE)
    manifest = {"schema": 2, "kind": "nes-auto-groups", "game_id": game_id,
                "recipe_key": pixel_recipe().key, "pack_scale": SCALE, "pages": ["group-page-000.png"],
                "groups": [{"key": pose.key, "cells": cells}]}
    # Publish the manifest last, in a generation no other playback is using.
    generation = uuid4().hex
    folder = owned_path(workspace, f"nes/packs/{generation}")
    folder.mkdir(parents=True)
    page.save(folder / manifest["pages"][0])
    atomic_json(folder / "manifest.json", {**manifest, "generation": generation})
    return generation


def _wait_for_close(session, process, log, *, overlay=False):
    if process.wait() or not session.status.is_file():
        raise RuntimeError(f"Mesen did not finish cleanly. See {log}")
    status = json.loads(session.status.read_bytes())
    if status.get("session_id") != session.session_id or status.get("phase") != "stopped":
        raise RuntimeError(f"Mesen did not finish cleanly. See {log}")
    if overlay and "[revival-managed] staged automatic sprite groups: 1" not in log.read_text(
            encoding="utf-8", errors="replace"):
        raise RuntimeError(f"Mesen did not stage the selected PNG. See {log}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    capture = actions.add_parser("capture", help="Open an isolated copy of a cartridge and collect placements.")
    capture.add_argument("rom", type=Path)
    review = actions.add_parser("review", help="Write previews for poses found at multiple positions.")
    review.add_argument("run", type=Path)
    play = actions.add_parser("play", help="Play with a selected pose regenerated or replaced by a PNG.")
    play.add_argument("run", type=Path)
    play.add_argument("pose_key")
    artwork = play.add_mutually_exclusive_group(required=True)
    artwork.add_argument("--png", type=Path)
    artwork.add_argument("--regenerate", action="store_true", help="Apply the existing Fast Pixel scaler to the whole pose.")
    args = parser.parse_args()

    if args.action == "capture":
        runtime = _runtime()
        game_id = NesRomInspector().inspect(args.rom).id
        run = Path(mkdtemp(prefix="nes-sprite-lab-", dir=BUILD))
        source = run / "nes" / "library" / game_id / "game.nes"
        workspace = run / "nes" / "rendered-games" / game_id
        source.parent.mkdir(parents=True)
        shutil.copyfile(args.rom, source)
        if NesRomInspector().inspect(source).id != game_id:
            raise ValueError("Cartridge changed while copying")
        session, process, log = _launch(runtime, game_id, source, workspace, capture=True)
        info = {"game_id": game_id, "capture_session": session.session_id}
        atomic_json(run / "run.json", info)
        print(f"Run: {run}\nPlay a moving section, then close Mesen normally. Log: {log}", flush=True)
        _wait_for_close(session, process, log)
        _review(run, info, workspace)
    elif args.action == "review":
        run, info, _, workspace = _paths(args.run)
        _review(run, info, workspace)
    else:
        runtime = _runtime()
        run, info, source, workspace = _paths(args.run)
        poses = moving_poses(workspace, info["game_id"], info["capture_session"])
        pose = next((pose for pose in poses if pose.key == args.pose_key), None)
        if pose is None:
            raise ValueError("Choose a pose key shown by the review command")
        if args.regenerate:
            replacement = enhance(pose.image())
        else:
            with Image.open(args.png, formats=["PNG"]) as image:
                if image.width > 4096 or image.height > 4096:
                    raise ValueError("Replacement PNG must fit within 4096×4096")
                replacement = image.convert("RGBA")
        generation = _stage_overlay(workspace, info["game_id"], pose, replacement)
        session, process, log = _launch(runtime, info["game_id"], source, workspace, generation=generation)
        print(f"Mesen session log: {log}", flush=True)
        _wait_for_close(session, process, log, overlay=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        raise SystemExit(f"NES sprite lab: {error}")
