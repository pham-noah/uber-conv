"""Regenerates the binary test fixtures with tools independent of uberconv.

    pip install pillow python-docx
    python3 make_fixtures.py

`example.heic` is not generated here: it is the sample image shipped with the
libheif project (strukturag/libheif, examples/example.heic), used to check our
HEIC decoding against an independent encoder.
"""
import io, os, stat, tarfile, time, zipfile
from PIL import Image, ImageOps
import docx

here = os.path.dirname(os.path.abspath(__file__))
out = lambda *p: os.path.join(here, *p)

# ── EXIF orientation: 8 images with the tag, plus PIL's answer for each ──
# Base image 6x4 with four distinguishable corners.
base = Image.new("RGB", (6, 4), (255, 255, 255))
for (x, y), c in {(0, 0): (255, 0, 0), (5, 0): (0, 255, 0), (0, 3): (0, 0, 255), (5, 3): (0, 0, 0)}.items():
    base.putpixel((x, y), c)
for o in range(1, 9):
    exif = Image.Exif()
    exif[0x0112] = o
    base.save(out(f"orient-{o}.png"), exif=exif)
    img = Image.open(out(f"orient-{o}.png"))
    ImageOps.exif_transpose(img).save(out(f"orient-{o}-expected.png"))

# ── 3-page PDF: red, green, blue pages ──
pages = [Image.new("RGB", (300, 200), c) for c in [(220, 30, 30), (30, 200, 60), (30, 60, 220)]]
pages[0].save(out("three-pages.pdf"), save_all=True, append_images=pages[1:], resolution=72)

# ── Word document made by python-docx ──
d = docx.Document()
d.add_heading("Quarterly report", level=1)
p = d.add_paragraph("Plain text with ")
p.add_run("bold").bold = True
p.add_run(", ")
p.add_run("italic").italic = True
p.add_run(" and a symbol: R&D <draft> *star*.")
d.add_heading("Details", level=2)
d.add_paragraph("first bullet", style="List Bullet")
d.add_paragraph("second bullet", style="List Bullet")
d.add_paragraph("step one", style="List Number")
d.add_paragraph("step two", style="List Number")
t = d.add_table(rows=2, cols=2)
t.rows[0].cells[0].text = "Name"; t.rows[0].cells[1].text = "Score"
t.rows[1].cells[0].text = "Ann";  t.rows[1].cells[1].text = "9"
d.save(out("sample.docx"))

# ── archives made by Python's stdlib ──
def tree():
    return [
        ("docs/", None, 0o755),
        ("docs/readme.txt", b"hello archive\n", 0o644),
        ("bin/run.sh", b"#!/bin/sh\necho hi\n", 0o755),
        ("ünï/çödé.txt", "unicode name".encode(), 0o600),
        ("big.bin", bytes(range(256)) * 2000, 0o644),
    ]
stamp = time.mktime((2021, 5, 17, 10, 30, 0, 0, 0, -1))
with zipfile.ZipFile(out("sample.zip"), "w", zipfile.ZIP_DEFLATED) as z:
    for name, data, mode in tree():
        zi = zipfile.ZipInfo(name, date_time=(2021, 5, 17, 10, 30, 0))
        zi.external_attr = ((stat.S_IFDIR if data is None else stat.S_IFREG) | mode) << 16
        zi.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(zi, data or b"")
    zi = zipfile.ZipInfo("docs/link", date_time=(2021, 5, 17, 10, 30, 0))
    zi.external_attr = (stat.S_IFLNK | 0o777) << 16
    z.writestr(zi, "readme.txt")
with tarfile.open(out("sample.tar.gz"), "w:gz") as t:
    for name, data, mode in tree():
        ti = tarfile.TarInfo(name.rstrip("/"))
        ti.mode, ti.mtime = mode, stamp
        if data is None:
            ti.type = tarfile.DIRTYPE; t.addfile(ti)
        else:
            ti.size = len(data); t.addfile(ti, io.BytesIO(data))
    ti = tarfile.TarInfo("docs/link"); ti.type = tarfile.SYMTYPE; ti.linkname = "readme.txt"; ti.mtime = stamp
    t.addfile(ti)
with zipfile.ZipFile(out("evil.zip"), "w") as z:
    z.writestr("../escape.txt", "nope")
print("fixtures written:", sorted(os.listdir(here)))
