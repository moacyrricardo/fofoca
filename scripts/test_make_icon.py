import runpy
import unittest
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

generator = runpy.run_path(str(Path(__file__).with_name("make-icon.py")))
TAURI = generator["TAURI"]


class IconTests(unittest.TestCase):
    def test_margin_preserves_enclosed_white_and_feathers_edge(self):
        art = Image.new("RGB", (15, 15), "white")
        draw = ImageDraw.Draw(art)
        draw.rectangle((2, 2, 12, 12), fill="#808890")
        draw.rectangle((3, 3, 11, 11), fill="#192e43")
        draw.rectangle((6, 6, 8, 8), fill="white")
        tile = generator["transparent_tile"](art)
        self.assertEqual(tile.getpixel((0, 0))[3], 0)
        self.assertEqual(tile.getpixel((7, 7)), (255, 255, 255, 255))
        self.assertTrue(0 < tile.getpixel((2, 7))[3] < 255)

    def test_desktop_png_corners_are_clear(self):
        for path in [TAURI / "app-icon.png", *(TAURI / "icons").glob("*.png")]:
            with self.subTest(icon=path.name), Image.open(path) as icon:
                self.assertEqual(icon.mode, "RGBA")
                for x in (0, icon.width - 1):
                    for y in (0, icon.height - 1):
                        self.assertEqual(icon.getpixel((x, y))[3], 0)

    def test_tray_images_match_source_and_recording_badge(self):
        with Image.open(TAURI / "app-icon.png") as tile:
            for name, expected in zip(("idle", "recording"), generator["tray_images"](tile)):
                with Image.open(TAURI / f"icons/tray-{name}.png") as icon:
                    self.assertEqual(icon.size, (64, 64))
                    self.assertIsNone(ImageChops.difference(icon, expected).getbbox(alpha_only=False))
            self.assertEqual(expected.getpixel((49, 49)), (253, 143, 110, 255))


if __name__ == "__main__":
    unittest.main()
