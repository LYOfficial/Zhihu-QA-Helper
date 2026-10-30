from pathlib import Path
from PIL import Image
import cairosvg

# 将品牌 SVG 渲染为 Tauri 所需的 Windows 图标尺寸
root = Path(__file__).resolve().parents[1]
svg = root / "apps" / "web" / "public" / "logo.svg"
out = root / "crates" / "desktop" / "icons"
out.mkdir(parents=True, exist_ok=True)
for name, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 256)]:
    cairosvg.svg2png(url=str(svg), write_to=str(out / name), output_width=size, output_height=size)
image = Image.open(out / "icon.png").convert("RGBA")
image.save(out / "icon.ico", format="ICO", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
