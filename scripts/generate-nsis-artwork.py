import os
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parents[1]
FONTS = Path(os.environ["WINDIR"]) / "Fonts"
LOGO = Image.open(ROOT / "crates/desktop/icons/icon.png").convert("RGBA")
BLUE = "#1761e7"


def font(size, bold=False):
    path = FONTS / ("msyhbd.ttc" if bold else "msyh.ttc")
    return ImageFont.truetype(str(path if path.exists() else FONTS / "simsun.ttc"), size)


def line(draw, points, fill, width=1):
    draw.line(points, fill=fill, width=width, joint="curve")


def paper(width, height, finished):
    image = Image.new("RGBA", (width, height))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((2, 2, width - 2, height - 2), radius=13, fill="#fcfdff",
                           outline="#aac8fc", width=2)
    line(draw, [(27, 21), (27, height - 21)], "#d5e3fd", 2)
    draw.rounded_rectangle((42, 30, width - 41, 42), radius=4, fill="#5186e9")
    for row, end in [(61, width - 23), (75, width - 35), (89, width - 50), (103, width - 40)]:
        line(draw, [(42, row), (end, row)], "#c8d9f7", 3)
    if finished:
        draw.ellipse((width - 76, height - 77, width - 22, height - 23), fill=BLUE)
        line(draw, [(width - 63, height - 49), (width - 52, height - 38),
                    (width - 34, height - 59)], "white", 5)
    else:
        line(draw, [(46, height - 30), (width - 38, height - 30)], "#d55f51", 4)
    return image


def hero(output, stage):
    finished = stage == "finish"
    width, height = 1100, 335
    image = Image.new("RGB", (width, height))
    draw = ImageDraw.Draw(image)
    for x in range(width):
        ratio = x / width
        color = tuple(round(a + (b - a) * ratio) for a, b in zip((12, 32, 72), (21, 93, 204)))
        line(draw, [(x, 0), (x, height)], color)
    for y in range(20, height, 24):
        for x in range(625, width, 24):
            draw.ellipse((x, y, x + 2, y + 2), fill="#5890e3")
    for radius in (150, 184, 221):
        draw.arc((867 - radius, 165 - radius, 867 + radius, 165 + radius), 208, 515,
                 fill="#5085d2", width=1)
    line(draw, [(0, height - 4), (width, height - 4)], "#4b9afa", 4)
    for angle, position, tint in [(-14, (820, 101), "#739de0"), (13, (787, 85), "#a7c5fa")]:
        sheet = Image.new("RGBA", (144, 188))
        ImageDraw.Draw(sheet).rounded_rectangle((0, 0, 140, 181), radius=12,
                                                fill=tint, outline="#bdd6fd", width=2)
        sheet = sheet.rotate(angle, resample=Image.Resampling.BICUBIC, expand=True)
        image.paste(sheet, position, sheet)
    sheet = paper(174, 195, finished).rotate(7, resample=Image.Resampling.BICUBIC, expand=True)
    image.paste(sheet, (770, 74), sheet)
    logo = LOGO.resize((64, 64), Image.Resampling.LANCZOS)
    image.paste(logo, (65, 70), logo)
    title, subtitle = {
        "setup": ("安装知乎答主助手", "选择安装位置后，即可开始。"),
        "progress": ("正在安装", "正在准备应用文件，请稍候。"),
        "finish": ("安装完成", "现在可以打开应用，继续你的写作。"),
        "maintenance": ("管理已有安装", "选择重新安装、更新或卸载。"),
    }[stage]
    draw.text((145, 103), "Windows x64" if stage == "setup" else "知乎答主助手",
              font=font(29, True), fill="#dfebff", anchor="lm")
    draw.text((65, 191), title, font=font(54, True), fill="white", anchor="lm")
    draw.text((69, 256), subtitle, font=font(26), fill="#cedfff", anchor="lm")
    image.save(output / f"{stage}.bmp")


def header(output):
    image = Image.new("RGB", (150, 57), "#f2f6ff")
    draw = ImageDraw.Draw(image)
    for x in range(15, 151, 9):
        for y in range(5, 58, 9):
            draw.point((x, y), fill="#a4bfff")
    logo = LOGO.resize((37, 37), Image.Resampling.LANCZOS)
    image.paste(logo, (97, 10), logo)
    draw.line((0, 56, 149, 56), fill="#d4e3fe")
    image.save(output / "installer-header.bmp")


if __name__ == "__main__":
    output = Path(sys.argv[1])
    output.mkdir(parents=True, exist_ok=True)
    for stage in ("setup", "progress", "finish", "maintenance"):
        hero(output, stage)
    header(output)
