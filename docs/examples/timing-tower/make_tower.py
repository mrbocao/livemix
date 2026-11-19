#!/usr/bin/env python3
"""Builds a LiveMix timing tower template (.gtzip) with no GT Designer needed.

The template follows the naming LiveMix reads for Live Timing:

  Row1 ... RowN        the rows (a Layer each); they slide when the order changes
  RowN.Pos / .No / .Name / .Time     text fields of a row
  Header.Event / .Run / .Class / .Mode   text fields outside the rows
  Cockpit.Frame        an empty frame: put a camera layer under the tower to fill it

Examples:
  py make_tower.py timing-tower.gtzip
  py make_tower.py my-tower.gtzip --rows 12 --left 68 --top 255 --header-image header.png \
      --row-odd line1.png --row-even line2.png --font "Exo 2"
"""
import argparse
import zipfile
from xml.sax.saxutils import quoteattr


def parse():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("output")
    p.add_argument("--rows", type=int, default=20)
    p.add_argument("--left", type=float, default=68)
    p.add_argument("--top", type=float, default=255, help="top of the first row")
    p.add_argument("--width", type=float, default=284, help="row width")
    p.add_argument("--pitch", type=float, default=34, help="distance between rows (and row height)")
    p.add_argument("--font", default="Exo 2")
    p.add_argument("--navy", default="#001638")
    p.add_argument("--navy-alt", default="#0a1f44")
    p.add_argument("--gold", default="#d4af37")
    p.add_argument("--no-colour", default="#c9a227", help="colour of the car number")
    p.add_argument("--time-colour", default="#7ecfef")
    p.add_argument("--header-image", help="PNG drawn instead of the built-in header panel (full 1920x1080 canvas)")
    p.add_argument("--row-odd", help="PNG for odd rows")
    p.add_argument("--row-even", help="PNG for even rows")
    p.add_argument("--cockpit", default="372,255,560,315", help="x,y,w,h of the cockpit frame, or 'none'")
    return p.parse_args()


def attrs(**values):
    return " ".join(f"{k}={quoteattr(str(v))}" for k, v in values.items())


def rect(name, x, y, w, h, fill, stroke=None, thickness=0):
    stroke_xml = f'<Rectangle.Stroke><Brush Color="{stroke}"/></Rectangle.Stroke>' if stroke else ""
    return (f'<Rectangle {attrs(Name=name, Location=f"{x:g},{y:g}", Dimensions=f"{w:g},{h:g}", StrokeThickness=thickness)}>'
            f'<Rectangle.Fill><Brush Color="{fill}"/></Rectangle.Fill>{stroke_xml}</Rectangle>')


def image(name, x, y, w, h, source):
    return (f'<Image {attrs(Name=name, Location=f"{x:g},{y:g}", Dimensions=f"{w:g},{h:g}")}>'
            f'<Image.Bitmap><Bitmap Source={quoteattr(source)}/></Image.Bitmap></Image>')


def text(name, sample, x, y, w, h, size, colour, font, align="Center", weight="Bold", shrink=True, wrap=False, visible=True):
    extra = {"AutoSize": "Shrink"} if shrink else {}
    if not visible:
        extra["Visible"] = "False"
    return (f'<TextBlock {attrs(Name=name, Text=sample, Location=f"{x:g},{y:g}", Dimensions=f"{w:g},{h:g}", FontFamily=font, FontSize=size, FontWeight=weight, TextAlign=align, VerticalAlign="Center", TextWordWrapping="Wrap" if wrap else "NoWrap", **extra)}>'
            f'<TextBlock.Fill><Brush Color="{colour}"/></TextBlock.Fill></TextBlock>')


def board(kind, animations, data_name=None):
    name = f' DataName="{data_name}"' if data_name else ""
    return f'<Storyboard Type="{kind}"{name}><Storyboard.Animations>{"".join(animations)}</Storyboard.Animations></Storyboard>'


def anim(effect, obj, delay=0.0, duration=0.3, **extra):
    return f'<{effect} {attrs(Object=obj, Delay=f"{delay:g}", Duration=f"{duration:g}", **extra)}/>'


def main():
    a = parse()
    f = a.font
    assets = {}
    parts = []
    # Header: built-in panel or a full-canvas picture, and the fields shown over it.
    if a.header_image:
        assets["header.png"] = a.header_image
        parts.append(image("Header.Art", 0, 0, 1920, 1080, "header.png"))
    else:
        parts += [
            rect("Header.Panel", a.left, 30, a.width, 190, a.navy),
            rect("Header.Strip", a.left, 176, a.width, 44, a.gold),
        ]
    bar_y = a.top - 31
    parts.append(rect("Header.ModeBar", a.left, bar_y, a.width, 26, a.gold))
    if a.header_image:
        # The picture carries the logo: keep the event and session fields (hidden) for templates
        # that want them, and share the gold bar between the class and the mode.
        parts += [
            text("Header.Event", "EVENT NAME", a.left + 10, 50, a.width - 20, 60, 22, "#ffffff", f, wrap=True, visible=False),
            text("Header.Run", "SESSION", a.left + 10, 112, a.width - 20, 28, 14, a.gold, f, visible=False),
            text("Header.Class", "ALL", a.left + 8, bar_y, 110, 26, 13, a.navy, f, align="Left"),
            text("Header.Mode", "LAP TIME", a.left + 120, bar_y, a.width - 128, 26, 13, a.navy, f, align="Right"),
        ]
    else:
        parts += [
            text("Header.Event", "EVENT NAME", a.left + 10, 50, a.width - 20, 60, 22, "#ffffff", f, wrap=True),
            text("Header.Run", "SESSION", a.left + 10, 112, a.width - 20, 28, 14, a.gold, f),
            text("Header.Class", "ALL", a.left, 176, a.width, 44, 20, a.navy, f),
            text("Header.Mode", "LAP TIME", a.left, bar_y, a.width, 26, 13, a.navy, f),
        ]
    if a.row_odd: assets["row_odd.png"] = a.row_odd
    if a.row_even: assets["row_even.png"] = a.row_even
    w, h = a.width, a.pitch
    for n in range(1, a.rows + 1):
        odd = n % 2 == 1
        if (a.row_odd if odd else a.row_even):
            background = image(f"Row{n}.Bg", 0, 0, w, h, "row_odd.png" if odd else "row_even.png")
        else:
            background = rect(f"Row{n}.Bg", 0, 0, w, h, a.navy_alt if odd else a.navy)
        inner = "".join([
            background,
            rect(f"Row{n}.Stripe", 0, 0, 4, h, a.gold),
            text(f"Row{n}.Pos", str(n), 6, 0, 30, h, 16, "#ffffff", f),
            text(f"Row{n}.No", "0", 40, 0, 34, h, 13, a.no_colour, f),
            text(f"Row{n}.Name", "DRIVER NAME", 78, 0, 128, h, 12, "#ffffff", f, align="Left"),
            text(f"Row{n}.Time", "0:00.000", 200, 0, 80, h, 12, a.time_colour, f, align="Right"),
        ])
        parts.append(f'<Layer {attrs(Name=f"Row{n}", Location=f"{a.left:g},{a.top + (n - 1) * a.pitch:g}", Dimensions=f"{w:g},{h:g}")}>'
                     f'<Layer.Composition><Composition Width="{w:g}" Height="{h:g}">{inner}</Composition></Layer.Composition></Layer>')
    if a.cockpit != "none":
        x, y, cw, ch = (float(v) for v in a.cockpit.split(","))
        parts += [rect("Cockpit.Frame", x, y, cw, ch, "#00000000", stroke=a.gold, thickness=3),
                  text("Cockpit.Label", "ONBOARD", x, y - 26, cw, 22, 12, a.gold, f, align="Left")]

    # Entrance and exit: header first, then the rows one after another.
    header_objects = ["Header.Panel", "Header.Strip", "Header.Art", "Header.Event", "Header.Run", "Header.Class", "Header.ModeBar", "Header.Mode"]
    enter = [anim("Fade", o, 0, 0.35) for o in header_objects]
    leave = [anim("Fade", o, 0, 0.25) for o in header_objects]
    for n in range(1, a.rows + 1):
        enter += [anim("Fade", f"Row{n}", 0.2 + 0.03 * n, 0.25), anim("Fly", f"Row{n}", 0.2 + 0.03 * n, 0.35, Direction="Top", Interpolation="CubicEasingOut")]
        leave += [anim("Fade", f"Row{n}", 0.02 * (a.rows - n), 0.2)]
    if a.cockpit != "none":
        enter += [anim("Fade", "Cockpit.Frame", 0.5, 0.4), anim("Fade", "Cockpit.Label", 0.5, 0.4)]
        leave += [anim("Fade", "Cockpit.Frame", 0, 0.2), anim("Fade", "Cockpit.Label", 0, 0.2)]
    boards = [board("TransitionIn", enter), board("TransitionOut", leave)]
    # A changed name, position or number fades out and back in. Times just update.
    fields = [f"Row{n}.{s}" for n in range(1, a.rows + 1) for s in ("Pos", "No", "Name")] + ["Header.Mode", "Header.Class"]
    for field in fields:
        boards.append(board("DataChangeOut", [anim("Fade", field, 0, 0.12)], f"{field}.Text"))
        boards.append(board("DataChangeIn", [anim("Fade", field, 0, 0.2)], f"{field}.Text"))

    document = f'<Composition Width="1920" Height="1080">{"".join(parts)}{"".join(boards)}</Composition>'
    # GT Designer stores UTF-16 with a byte order mark; LiveMix reads both.
    with zipfile.ZipFile(a.output, "w", zipfile.ZIP_DEFLATED) as package:
        package.writestr("document.xml", b"\xff\xfe" + document.encode("utf-16-le"))
        for name, path in assets.items():
            package.write(path, name)
    print(f"{a.output}: {a.rows} rows, pitch {a.pitch:g}, {len(fields)} animated fields")


if __name__ == "__main__":
    main()
