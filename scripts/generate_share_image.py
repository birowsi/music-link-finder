"""Rebuild the share card: python scripts/generate_share_image.py (Pillow needed).

The checked-in PNG is served directly; Python is not needed to build/run UtaLink.
All artwork is drawn here, with the project's bundled Pretendard JP fonts.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parents[1]
SCALE = 3
WIDTH, HEIGHT = 1200, 630
BG = "#FFF7FA"
INK = "#3D3340"
MUTED = "#7D6878"
ACCENT = "#E5A0BB"
ACCENT_TEXT = "#A5436D"


def box(values):
    return tuple(round(value * SCALE) for value in values)


def font(size, bold=False):
    name = "PretendardJP-SemiBold.otf" if bold else "PretendardJP-Regular.otf"
    return ImageFont.truetype(str(ROOT / "assets" / "fonts" / name), size * SCALE)


def generate():
    image = Image.new("RGB", (WIDTH * SCALE, HEIGHT * SCALE), BG)
    draw = ImageDraw.Draw(image)
    draw.ellipse(box((920, -270, 1480, 350)), fill="#F8E1EB")
    draw.ellipse(box((-180, 510, 170, 860)), fill="#FBEAF1")
    draw.rounded_rectangle(box((64, 72, 1136, 574)), radius=30 * SCALE, fill="#F2DEE8")
    draw.rounded_rectangle(
        box((64, 64, 1136, 566)), radius=30 * SCALE,
        fill="#FFFCFD", outline="#E9D8E1", width=SCALE,
    )

    draw.rounded_rectangle(box((116, 116, 123, 143)), radius=3 * SCALE, fill=ACCENT)
    draw.text(box((139, 114)), "음악 링크 찾기", font=font(22), fill=MUTED)
    draw.text(box((108, 169)), "UtaLink", font=font(108, bold=True), fill=INK)
    draw.text(box((116, 316)), "한글 발음 · 노래방 반주", font=font(34, bold=True), fill=INK)
    draw.text(box((116, 377)), "검색하고, 선택하고, 링크를 복사하세요.", font=font(24), fill=MUTED)

    # A pair of linked loops, drawn from geometry rather than external artwork.
    tile = Image.new("RGBA", (264 * SCALE, 264 * SCALE))
    tile_draw = ImageDraw.Draw(tile)
    tile_draw.rounded_rectangle(box((0, 0, 264, 264)), radius=52 * SCALE, fill="#FAEAF2")
    link = Image.new("RGBA", tile.size)
    link_draw = ImageDraw.Draw(link)
    for bounds in [(34, 95, 151, 169), (113, 95, 230, 169)]:
        link_draw.rounded_rectangle(box(bounds), radius=37 * SCALE, outline=ACCENT_TEXT, width=11 * SCALE)
    link_draw.rectangle(box((106, 106, 158, 158)), fill="#FAEAF2")
    link_draw.line(box((92, 132, 172, 132)), fill=ACCENT_TEXT, width=11 * SCALE)
    link = link.rotate(38, resample=Image.Resampling.BICUBIC)
    tile.alpha_composite(link)
    image.paste(tile, box((808, 180)), tile)
    draw.ellipse(box((1034, 158, 1094, 218)), fill=ACCENT)
    draw.line(box((1068, 173, 1068, 196)), fill=INK, width=3 * SCALE)
    draw.line(box((1068, 173, 1078, 170)), fill=INK, width=3 * SCALE)
    draw.ellipse(box((1058, 191, 1069, 200)), fill=INK)

    # A small slanted signature, using the same bundled font for repeatability.
    credit = Image.new("RGBA", (145 * SCALE, 35 * SCALE))
    ImageDraw.Draw(credit).text((8 * SCALE, 0), "by hanbi.", font=font(20), fill=MUTED)
    credit = credit.transform(
        credit.size, Image.Transform.AFFINE, (1, 0.18, -6 * SCALE, 0, 1, 0),
        resample=Image.Resampling.BICUBIC,
    )
    image.paste(credit, box((108, 494)), credit)
    draw.text(box((838, 498)), "YouTube 음악 검색", font=font(20), fill=MUTED)

    output = ROOT / "web" / "og-image-v1.png"
    image.resize((WIDTH, HEIGHT), Image.Resampling.LANCZOS).save(output, optimize=True)
    print(f"{output}: {WIDTH}x{HEIGHT}, {output.stat().st_size:,} bytes")


if __name__ == "__main__":
    generate()
