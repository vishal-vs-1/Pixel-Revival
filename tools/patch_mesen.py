"""Generate bridge sources for the pinned MesenCE checkout."""

import argparse
from pathlib import Path
import sys

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.patches.mesen.generate import generate


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--inprocess", action="store_true")
    args = parser.parse_args()
    generate(args.source_root.resolve(), args.output_root.resolve(), inprocess=args.inprocess)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
