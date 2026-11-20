#!/usr/bin/env python3
"""Prepare the seal artwork and tray images. Then run cargo tauri icon app-icon.png
in app/src-tauri to rebuild the desktop bundle icons.
"""
import argparse
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageOps

TAURI = Path(__file__).resolve().parents[1] / "app/src-tauri"


def transparent_tile(source):
    tile = source.convert("RGBA")
    if tile.getextrema()[3][0] == 0:
        return tile  # Regenerating the tray images must not erode a finished tile.
    rgb = tile.convert("RGB")
    margin = rgb.convert("L").point(lambda value: 255 if value >= 200 else 0)
    w, h = tile.size
    for corner in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]:
        if margin.getpixel(corner) == 255:
            ImageDraw.floodfill(margin, corner, 128)
    alpha = margin.point(lambda value: 0 if value == 128 else 255)
    # Remove the white matte in the one-pixel rim. Estimate the original edge
    # colour from the darkest adjacent tile pixel (the tile has a navy border).
    rim = ImageChops.subtract(alpha, alpha.filter(ImageFilter.MinFilter(3)))
    box = rim.getbbox()
    if box:
        for y in range(box[1], box[3]):
            for x in range(box[0], box[2]):
                if rim.getpixel((x, y)):
                    neighbours = (rgb.getpixel((xx, yy))
                                  for yy in range(max(0, y - 1), min(h, y + 2))
                                  for xx in range(max(0, x - 1), min(w, x + 2)))
                    edge = min(neighbours, key=sum)
                    opacity = (765 - sum(rgb.getpixel((x, y)))) / max(1, 765 - sum(edge))
                    alpha.putpixel((x, y), round(255 * opacity))
                    tile.putpixel((x, y), (*edge, 255))
    tile.putalpha(alpha)
    return tile


def tray_images(tile):
    idle = tile.resize((64, 64), Image.Resampling.LANCZOS)
    badge = Image.new("RGBA", (256, 256))
    ImageDraw.Draw(badge).ellipse((148, 148, 243, 243), fill="#fd8f6e", outline="#192e43", width=10)
    return idle, Image.alpha_composite(idle, badge.resize(idle.size, Image.Resampling.LANCZOS))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artwork", nargs="?", type=Path, default=TAURI / "app-icon.png")
    args = parser.parse_args()
    with Image.open(args.artwork) as source:
        tile = transparent_tile(source)
        # A half-pixel gutter at 32px prevents resize ringing in bundle corners.
        if "A" not in source.getbands() or source.getchannel("A").getextrema()[0] == 255:
            tile = ImageOps.expand(tile, border=max(1, round(tile.width / 64)))
    tile.save(TAURI / "app-icon.png")
    idle, recording = tray_images(tile)
    idle.save(TAURI / "icons/tray-idle.png")
    recording.save(TAURI / "icons/tray-recording.png")
