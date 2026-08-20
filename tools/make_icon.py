"""Generate assets/quotty.ico from the same design as the in-app tray icon
(src/icon.rs): a rounded dark tile with two quota bars.

    python tools/make_icon.py
"""
from PIL import Image, ImageDraw

S = 1024  # drawn large, downscaled per icon size for clean antialiasing
BG = (26, 28, 34, 255)
TRACK = (60, 64, 74, 255)
BARS = [(0.75, (90, 150, 255, 255)), (0.40, (120, 200, 160, 255))]

img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle([0, 0, S - 1, S - 1], radius=int(S * 0.1875), fill=BG)

left, right = S * 0.1875, S * 0.8125
bar_h = S * 0.125
for i, (frac, col) in enumerate(BARS):
    y0 = S * (9 + i * 9) / 32
    r = bar_h / 2
    d.rounded_rectangle([left, y0, right, y0 + bar_h], radius=r, fill=TRACK)
    d.rounded_rectangle([left, y0, left + (right - left) * frac, y0 + bar_h],
                        radius=r, fill=col)

sizes = [16, 24, 32, 48, 64, 128, 256]
img.save("assets/quotty.ico", sizes=[(s, s) for s in sizes])
img.resize((256, 256), Image.LANCZOS).save("assets/quotty.png")
print("wrote assets/quotty.ico and assets/quotty.png")
