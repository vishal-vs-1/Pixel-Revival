"""Preview likely autonomous NES sprites from a saved gameplay-hook journal."""

import argparse
import json
from pathlib import Path

from PIL import Image, ImageDraw

from systems.nes.enemy_candidates import find_enemy_candidates
from systems.nes.live_player_capture import read_frames
from systems.nes.pose_layouts import MASTER_PALETTE


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("journal", type=Path)
    parser.add_argument("output", type=Path, help="New directory for review results")
    parser.add_argument("--chr-ram", action="store_true")
    args = parser.parse_args()
    candidates = find_enemy_candidates(read_frames(args.journal), ram=args.chr_ram)
    args.output.mkdir(parents=True)
    rows = []
    sheet = Image.new("RGB", (640, max(1, (len(candidates) + 7) // 8) * 86), "#242424")
    draw = ImageDraw.Draw(sheet)
    for index, candidate in enumerate(candidates):
        sprite = candidate.pose.image(MASTER_PALETTE)
        sprite = sprite.resize((sprite.width * 2, sprite.height * 2), Image.Resampling.NEAREST)
        sprite.thumbnail((74, 66), Image.Resampling.NEAREST)
        x, y = index % 8 * 80, index // 8 * 86
        sheet.paste(sprite, (x, y), sprite)
        draw.text((x, y + 67), str(index + 1), fill="white")
        rows.append({"id": index + 1, "pose": candidate.pose.document(),
                     "observations": candidate.observations,
                     "quiet_motion": candidate.quiet_motion, "active_motion": candidate.active_motion})
    if not candidates:
        draw.text((8, 8), "No candidates met the motion and player exclusion checks.", fill="white")
    sheet.save(args.output / "candidates.png")
    (args.output / "candidates.json").write_text(json.dumps(rows, indent=2), encoding="utf-8")
    print(f"{len(candidates)} candidates for review")


if __name__ == "__main__":
    main()
