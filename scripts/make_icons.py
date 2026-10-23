"""Render Clawd's idle frame into the icon set used by Tauri (src-tauri/icons)."""
from pathlib import Path

from PIL import Image

BODY = (0xD7, 0x77, 0x57, 255)
EYE = (0x00, 0x00, 0x00, 255)
GRID = 18

# (x0, y0, x1, y1, color) in grid cells, same as the idle frame in src/sprite.js.
RECTS = [
    *[(c, 11, c + 1, 13, BODY) for c in (4, 6, 11, 13)],
    (1, 7, 3, 9, BODY),
    (15, 7, 17, 9, BODY),
    (3, 3, 15, 11, BODY),
    (5, 5, 6, 7, EYE),
    (12, 5, 13, 7, EYE),
]
TOP_PAD = 2


def render(size: int) -> Image.Image:
    cell = size / GRID
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    px = img.load()
    for x0, y0, x1, y1, color in RECTS:
        for y in range(round((y0 + TOP_PAD) * cell), round((y1 + TOP_PAD) * cell)):
            for x in range(round(x0 * cell), round(x1 * cell)):
                px[x, y] = color
    return img


def main() -> None:
    out = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
    out.mkdir(parents=True, exist_ok=True)
    render(32).save(out / "32x32.png")
    render(128).save(out / "128x128.png")
    render(256).save(out / "128x128@2x.png")
    render(512).save(out / "icon.png")
    big = render(288)
    big.save(out / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])


if __name__ == "__main__":
    main()
