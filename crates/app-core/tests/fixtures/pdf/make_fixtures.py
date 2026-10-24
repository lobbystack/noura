"""Writes the PDF fixtures for Noura's file viewer tests.

text.pdf: three Letter pages of Helvetica text.
large.pdf: 400 pages that share one dense drawing, slow to render.
locked.pdf: two pages behind the user password "open sesame", encrypted
with the standard security handler, revision 3, 128-bit RC4.
slivers.pdf: three pages that draw less than a pixel on one side or both:
14400 by 1 points, 1 by 14400 points, and 1 by 1 point.
empty.pdf: a PDF with no pages.

Standard library only. Run: python3 make_fixtures.py OUTPUT_DIR
"""

import hashlib
import sys
import zlib
from pathlib import Path

PADDING = bytes.fromhex(
    "28BF4E5E4E758A4164004E56FFFA01082E2E00B6D0683E802F0CA9FE6453697A"
)


def rc4(key: bytes, data: bytes) -> bytes:
    s = list(range(256))
    j = 0
    for i in range(256):
        j = (j + s[i] + key[i % len(key)]) % 256
        s[i], s[j] = s[j], s[i]
    out = bytearray()
    i = j = 0
    for byte in data:
        i = (i + 1) % 256
        j = (j + s[i]) % 256
        s[i], s[j] = s[j], s[i]
        out.append(byte ^ s[(s[i] + s[j]) % 256])
    return bytes(out)


def pad(password: bytes) -> bytes:
    return (password + PADDING)[:32]


class Encryption:
    """The standard security handler, revision 3, 128-bit RC4."""

    def __init__(self, user: bytes, owner: bytes, file_id: bytes):
        self.file_id = file_id
        self.permissions = -4  # every permission
        length = 16
        # Algorithm 3: the O value.
        digest = hashlib.md5(pad(owner)).digest()
        for _ in range(50):
            digest = hashlib.md5(digest).digest()
        owner_key = digest[:length]
        o = rc4(owner_key, pad(user))
        for round_ in range(1, 20):
            o = rc4(bytes(b ^ round_ for b in owner_key), o)
        self.o = o
        # Algorithm 2: the file key.
        md5 = hashlib.md5()
        md5.update(pad(user))
        md5.update(o)
        md5.update((self.permissions & 0xFFFFFFFF).to_bytes(4, "little"))
        md5.update(file_id)
        digest = md5.digest()
        for _ in range(50):
            digest = hashlib.md5(digest[:length]).digest()
        self.key = digest[:length]
        # Algorithm 5: the U value.
        u = rc4(self.key, hashlib.md5(PADDING + file_id).digest())
        for round_ in range(1, 20):
            u = rc4(bytes(b ^ round_ for b in self.key), u)
        self.u = u + bytes(16)

    def object_key(self, number: int, generation: int) -> bytes:
        md5 = hashlib.md5()
        md5.update(self.key)
        md5.update(number.to_bytes(3, "little"))
        md5.update(generation.to_bytes(2, "little"))
        return md5.digest()[: min(len(self.key) + 5, 16)]

    def dictionary(self) -> bytes:
        return (
            b"<< /Filter /Standard /V 2 /R 3 /Length 128 /P %d /O <%s> /U <%s> >>"
            % (self.permissions, self.o.hex().encode(), self.u.hex().encode())
        )


def text_stream(lines) -> bytes:
    ops = [b"BT /F1 24 Tf 72 700 Td 30 TL"]
    for line in lines:
        escaped = line.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
        ops.append(b"(%s) Tj T*" % escaped.encode("latin-1"))
    ops.append(b"ET")
    return b"\n".join(ops)


def dense_drawing() -> bytes:
    ops = []
    for row in range(120):
        for column in range(90):
            gray = ((row * 7 + column * 13) % 100) / 100
            ops.append(
                b"%.2f g %d %d 6 6 re f" % (gray, 6 + column * 6.6, 10 + row * 6.4)
            )
    return b"\n".join(ops)


def write_pdf(
    path: Path,
    page_streams,
    shared=False,
    encryption=None,
    file_id=None,
    sizes=None,
):
    """`page_streams` holds one content stream per page, or with `shared`
    one stream that every page draws. `sizes` holds each page's width and
    height in points, Letter by default."""
    objects = {}
    number = iter(range(1, 100000))
    catalog = next(number)
    pages = next(number)
    font = next(number)
    objects[font] = (
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica "
        b"/Encoding /WinAnsiEncoding >>"
    )
    streams = [page_streams] if shared else page_streams
    stream_numbers = []
    for content in streams:
        n = next(number)
        stream_numbers.append(n)
        objects[n] = ("stream", zlib.compress(content, 9))
    count = len(page_streams) if not shared else shared
    kids = []
    for index in range(count):
        n = next(number)
        kids.append(n)
        contents = stream_numbers[0] if shared else stream_numbers[index]
        width, height = sizes[index] if sizes else (612, 792)
        objects[n] = (
            b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 %d %d] "
            b"/Resources << /Font << /F1 %d 0 R >> >> /Contents %d 0 R >>"
            % (pages, width, height, font, contents)
        )
    objects[pages] = b"<< /Type /Pages /Kids [%s] /Count %d >>" % (
        b" ".join(b"%d 0 R" % kid for kid in kids),
        count,
    )
    objects[catalog] = b"<< /Type /Catalog /Pages %d 0 R >>" % pages
    encrypt_number = None
    if encryption:
        encrypt_number = next(number)
        objects[encrypt_number] = encryption.dictionary()

    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = {}
    for n in sorted(objects):
        offsets[n] = len(out)
        body = objects[n]
        out += b"%d 0 obj\n" % n
        if isinstance(body, tuple):
            data = body[1]
            if encryption and n != encrypt_number:
                data = rc4(encryption.object_key(n, 0), data)
            out += b"<< /Length %d /Filter /FlateDecode >>\nstream\n" % len(data)
            out += data + b"\nendstream"
        else:
            out += body
        out += b"\nendobj\n"
    xref = len(out)
    size = max(objects) + 1
    out += b"xref\n0 %d\n0000000000 65535 f \n" % size
    for n in range(1, size):
        out += b"%010d 00000 n \n" % offsets[n]
    trailer = b"<< /Size %d /Root %d 0 R" % (size, catalog)
    if file_id:
        trailer += b" /ID [<%s> <%s>]" % (file_id.hex().encode(), file_id.hex().encode())
    if encrypt_number:
        trailer += b" /Encrypt %d 0 R" % encrypt_number
    trailer += b" >>"
    out += b"trailer\n" + trailer + b"\nstartxref\n%d\n%%%%EOF\n" % xref
    path.write_bytes(bytes(out))


def main():
    folder = Path(sys.argv[1])
    folder.mkdir(parents=True, exist_ok=True)
    write_pdf(
        folder / "text.pdf",
        [
            text_stream(["First page.", "An apple a day."]),
            text_stream(["Second page.", "No fruit here."]),
            text_stream(["Third page.", "Apple and apple again."]),
        ],
    )
    write_pdf(folder / "large.pdf", dense_drawing(), shared=400)
    file_id = hashlib.md5(b"noura locked fixture").digest()
    write_pdf(
        folder / "locked.pdf",
        [
            text_stream(["Locked first page."]),
            text_stream(["Locked second page."]),
        ],
        encryption=Encryption(b"open sesame", b"owner of noura", file_id),
        file_id=file_id,
    )
    sizes = [(14400, 1), (1, 14400), (1, 1)]
    write_pdf(
        folder / "slivers.pdf",
        [b"0 g 0 0 %d %d re f" % size for size in sizes],
        sizes=sizes,
    )
    write_pdf(folder / "empty.pdf", [])


if __name__ == "__main__":
    main()
