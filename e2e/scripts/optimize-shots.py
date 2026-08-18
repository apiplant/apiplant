#!/usr/bin/env python3
"""Shrink the screenshots in docs/images/ before they are committed.

Playwright writes true-colour PNGs, and a 2× shot of the studio is most of a
megabyte of that. A dashboard is flat colour — a few hundred distinct values
across the whole frame — so quantising to a 256-entry palette is invisible at
the sizes these are read at and costs roughly two thirds of the bytes.

Idempotent: an already-quantised file has a palette and is left alone, so
running this twice does not degrade anything.

Pillow is the only dependency, and its absence is not an error: the pictures
are correct either way, just larger.
"""

from pathlib import Path
import sys

IMAGES = Path(__file__).resolve().parents[2] / "docs" / "images"

try:
    from PIL import Image
except ImportError:
    print("optimize-shots: Pillow is not installed; leaving the PNGs as taken")
    sys.exit(0)


def main() -> None:
    if not IMAGES.is_dir():
        print(f"optimize-shots: no {IMAGES}")
        return

    saved = 0
    for path in sorted(IMAGES.glob("*.png")):
        before = path.stat().st_size
        with Image.open(path) as image:
            if image.mode == "P":
                continue
            palette = image.convert("RGB").quantize(colors=256, dither=Image.Dither.NONE)
            palette.save(path, optimize=True)
        after = path.stat().st_size
        saved += before - after
        print(f"  {path.name}  {before // 1024} KB → {after // 1024} KB")

    print(f"optimize-shots: saved {saved // 1024} KB")


if __name__ == "__main__":
    main()
