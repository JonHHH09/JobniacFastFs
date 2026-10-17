"""Generate the Standard-14 width tables of jobniac-pdf from the Adobe Core14 AFM files.

The tables are derived directly from the Adobe AFM files, which Apache PDFBox 3.0.7 ships in
``org/apache/pdfbox/resources/afm`` inside ``pdfbox-3.0.7.jar``. This script reads only the AFM
files and the Adobe ``MustRead.html`` notice; it never executes anything from them.

Outputs:

* ``crates/pdf/src/metrics/tables.rs``: generated Rust tables (generic Helvetica and Helvetica-Bold
  widths for the 192 WinAnsi glyphs in WinAnsi index order, and the 26 extra Windows-1252
  glyphs of the German encoding). The header names the source jar, its version and SHA-256, the
  AFM file digests, and the Adobe notice verbatim, and marks the file as a modification (derivation)
  of the AFM data.
* ``crates/pdf/src/metrics/NOTICE-Adobe-AFM.txt``: the Adobe notice, verbatim (``--notice-out``).

Index order of the generic tables (matches ``src/jobniac/infrastructure/pdf_font_metrics.py``):
code points 32..126 map to index ``cp - 32``, code points 160..255 map to ``cp - 65`` and the
generated bullet U+2022 maps to index 191. Two aliases follow PDFBox: NBSP (U+00A0) takes the width
of ``space`` and the soft hyphen (U+00AD) the width of ``hyphen``.

The generated file is then formatted with ``cargo fmt`` (the committed tables.rs is rustfmt output).

Usage (from the repository root, stdlib only)::

    py -I tools/fonts/afm_tables.py --afm-dir <folder with the AFM files> \\
        --jar <pdfbox-3.0.7.jar> --pdfbox-version 3.0.7 \\
        --out crates/pdf/src/metrics/tables.rs \\
        --notice-out crates/pdf/src/metrics/NOTICE-Adobe-AFM.txt
"""

from __future__ import annotations

import argparse
import hashlib
import html.parser
import sys
from pathlib import Path

GENERATED_BULLET = 0x2022
BULLET_INDEX = 191
FACES = {'HELVETICA': 'Helvetica.afm', 'HELVETICA_BOLD': 'Helvetica-Bold.afm'}

# Glyph names of WinAnsi code points 32..126 (PDF specification, Annex D), ascending.
ASCII_NAMES = (
    'space exclam quotedbl numbersign dollar percent ampersand quotesingle '
    'parenleft parenright asterisk plus comma hyphen period slash '
    'zero one two three four five six seven eight nine colon semicolon '
    'less equal greater question at '
    'A B C D E F G H I J K L M N O P Q R S T U V W X Y Z '
    'bracketleft backslash bracketright asciicircum underscore grave '
    'a b c d e f g h i j k l m n o p q r s t u v w x y z '
    'braceleft bar braceright asciitilde'
).split()

# Glyph names of Unicode code points 160..255 (identical to Latin-1 and WinAnsi), ascending.
LATIN1_NAMES = (
    'space exclamdown cent sterling currency yen brokenbar section '
    'dieresis copyright ordfeminine guillemotleft logicalnot hyphen registered macron '
    'degree plusminus twosuperior threesuperior acute mu paragraph periodcentered '
    'cedilla onesuperior ordmasculine guillemotright onequarter onehalf threequarters questiondown '
    'Agrave Aacute Acircumflex Atilde Adieresis Aring AE Ccedilla '
    'Egrave Eacute Ecircumflex Edieresis Igrave Iacute Icircumflex Idieresis '
    'Eth Ntilde Ograve Oacute Ocircumflex Otilde Odieresis multiply '
    'Oslash Ugrave Uacute Ucircumflex Udieresis Yacute Thorn germandbls '
    'agrave aacute acircumflex atilde adieresis aring ae ccedilla '
    'egrave eacute ecircumflex edieresis igrave iacute icircumflex idieresis '
    'eth ntilde ograve oacute ocircumflex otilde odieresis divide '
    'oslash ugrave uacute ucircumflex udieresis yacute thorn ydieresis'
).split()

# Windows-1252 glyphs outside the generic set (code point, glyph name), ascending.
GERMAN_EXTRA_GLYPHS = (
    (338, 'OE'),
    (339, 'oe'),
    (352, 'Scaron'),
    (353, 'scaron'),
    (376, 'Ydieresis'),
    (381, 'Zcaron'),
    (382, 'zcaron'),
    (402, 'florin'),
    (710, 'circumflex'),
    (732, 'tilde'),
    (8211, 'endash'),
    (8212, 'emdash'),
    (8216, 'quoteleft'),
    (8217, 'quoteright'),
    (8218, 'quotesinglbase'),
    (8220, 'quotedblleft'),
    (8221, 'quotedblright'),
    (8222, 'quotedblbase'),
    (8224, 'dagger'),
    (8225, 'daggerdbl'),
    (8230, 'ellipsis'),
    (8240, 'perthousand'),
    (8249, 'guilsinglleft'),
    (8250, 'guilsinglright'),
    (8364, 'Euro'),
    (8482, 'trademark'),
)

# PDFBox aliases: NBSP is measured as space, the soft hyphen as hyphen.
ALIASES = {'nbspace': 'space', 'sfthyphen': 'hyphen'}


class _NoticeParser(html.parser.HTMLParser):
    """Collects the visible text of the notice cell and drops the hidden white-text fragments."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.in_cell = False
        self.hidden_depth = 0
        self.parts: list[str] = []

    def handle_starttag(self, tag, attrs):  # type: ignore[no-untyped-def]
        attributes = dict(attrs)
        if tag == 'td' and attributes.get('width') == '300':
            self.in_cell = True
        elif self.in_cell and tag == 'font' and attributes.get('color') == 'white':
            self.hidden_depth += 1

    def handle_endtag(self, tag):  # type: ignore[no-untyped-def]
        if tag == 'td' and self.in_cell:
            self.in_cell = False
        elif self.in_cell and tag == 'font' and self.hidden_depth:
            self.hidden_depth -= 1

    def handle_data(self, data):  # type: ignore[no-untyped-def]
        if self.in_cell and not self.hidden_depth:
            self.parts.append(data)


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _parse_afm(path: Path) -> tuple[dict[str, int], str]:
    """Return the glyph-name to WX map of one AFM file and its ``Notice`` line (verbatim)."""
    widths: dict[str, int] = {}
    notice = ''
    for raw in path.read_bytes().decode('ascii').split('\n'):
        line = raw.rstrip('\r')
        if line.startswith('Notice '):
            notice = line[len('Notice ') :]
        if not line.startswith('C '):
            continue
        fields = [field.strip() for field in line.split(';')]
        wx = None
        name = None
        for field in fields:
            parts = field.split(' ')
            if parts[0] == 'WX':
                wx = int(parts[1])
            elif parts[0] == 'N':
                name = parts[1]
        if wx is None or name is None:
            raise SystemExit(f'{path.name}: malformed glyph line: {line!r}')
        if name in widths and widths[name] != wx:
            raise SystemExit(f'{path.name}: conflicting widths for {name}')
        widths[name] = wx
    if not notice:
        raise SystemExit(f'{path.name}: no Notice line')
    return widths, notice


def _lookup(widths: dict[str, int], name: str, face: str) -> int:
    resolved = ALIASES.get(name, name)
    if resolved not in widths:
        raise SystemExit(f'{face}: glyph {resolved!r} missing from the AFM file')
    return widths[resolved]


def _generic_table(widths: dict[str, int], face: str) -> list[int]:
    names = list(ASCII_NAMES) + list(LATIN1_NAMES) + ['bullet']
    if len(names) != 192 or names[BULLET_INDEX] != 'bullet':
        raise SystemExit('internal error: generic glyph order')
    return [_lookup(widths, name, face) for name in names]


def _rows(values: list[int], per_row: int = 16) -> list[str]:
    lines = []
    for start in range(0, len(values), per_row):
        chunk = values[start : start + per_row]
        lines.append('    ' + ' '.join(f'{value},' for value in chunk))
    return lines


def _notice_paragraph(path: Path) -> str:
    parser = _NoticeParser()
    parser.feed(path.read_bytes().decode('iso-8859-1'))
    text = ''.join(parser.parts)
    return ' '.join(text.split())


def _comment(text: str, prefix: str = '//') -> list[str]:
    return [f'{prefix} {line}'.rstrip() if line else prefix for line in text.split('\n')]


def build(afm_dir: Path, jar: Path | None, pdfbox_version: str, out: Path, notice_out: Path) -> None:
    """Parse the AFM files and write the Rust tables and the notice file."""
    parsed = {}
    for face, file_name in FACES.items():
        parsed[face] = _parse_afm(afm_dir / file_name)
    notice_line = parsed['HELVETICA'][1]
    if notice_line != parsed['HELVETICA_BOLD'][1]:
        raise SystemExit('the two AFM files carry different notices')
    paragraph = _notice_paragraph(afm_dir / 'MustRead.html')
    if not paragraph:
        raise SystemExit('MustRead.html notice paragraph not found')
    notice_text = (
        f'{notice_line}\n\n'
        'Adobe Core14 AFM notice (MustRead.html of the AFM set; the hidden white-text fragment of the\n'
        'HTML page is omitted):\n\n'
        f'{paragraph}\n'
    )

    normal = _generic_table(parsed['HELVETICA'][0], 'Helvetica')
    bold = _generic_table(parsed['HELVETICA_BOLD'][0], 'Helvetica-Bold')
    extras = []
    for codepoint, name in GERMAN_EXTRA_GLYPHS:
        extras.append(
            (
                codepoint,
                name,
                _lookup(parsed['HELVETICA'][0], name, 'Helvetica'),
                _lookup(parsed['HELVETICA_BOLD'][0], name, 'Helvetica-Bold'),
            )
        )

    header = [
        '// @generated by tools/fonts/afm_tables.py from the Adobe Core14 AFM files. Do not edit.',
        '//',
        '// MODIFICATION NOTICE: these tables are a modification (derivation) of Adobe AFM glyph-width',
        '// data. They keep the glyph widths of the Helvetica and Helvetica-Bold AFM files only, reordered',
        '// into the WinAnsi index order, with the PDFBox aliases listed below.',
        '//',
        '// Source archive: pdfbox-' + pdfbox_version + '.jar, org/apache/pdfbox/resources/afm/ (Maven Central,',
        '// Apache PDFBox ' + pdfbox_version + ').',
    ]
    if jar is not None:
        header.append(f'// Jar SHA-256: {_sha256(jar)}')
    header += [
        f'// Helvetica.afm SHA-256: {_sha256(afm_dir / "Helvetica.afm")}',
        f'// Helvetica-Bold.afm SHA-256: {_sha256(afm_dir / "Helvetica-Bold.afm")}',
        '// Aliases: U+00A0 uses the width of space; U+00AD uses the width of hyphen.',
        '//',
        '// Adobe notice (verbatim; also in NOTICE-Adobe-AFM.txt):',
    ]
    header += _comment(notice_text)
    header += [
        '',
        '//! Standard-14 Helvetica width tables (generated; see the header of this file).',
        '',
        '/// Helvetica advance widths in 1000-unit space for the generic WinAnsi glyph set.',
        '///',
        '/// Index: U+0020..=U+007E at `cp - 32`, U+00A0..=U+00FF at `cp - 65`, and the generated',
        '/// bullet U+2022 at 191.',
        f'pub(crate) const HELVETICA_WIDTHS: [u16; {len(normal)}] = [',
        *_rows(normal),
        '];',
        '',
        '/// Helvetica-Bold advance widths in 1000-unit space, same index order as `HELVETICA_WIDTHS`.',
        f'pub(crate) const HELVETICA_BOLD_WIDTHS: [u16; {len(bold)}] = [',
        *_rows(bold),
        '];',
        '',
        '/// Windows-1252 glyphs outside the generic set: (code point, normal width, bold width),',
        '/// in ascending code point order.',
        f'pub(crate) const GERMAN_EXTRA_WIDTHS: [(u32, u16, u16); {len(extras)}] = [',
    ]
    for codepoint, name, normal_width, bold_width in extras:
        header.append(f'    ({codepoint}, {normal_width}, {bold_width}), // {name}')
    header.append('];')
    header.append('')

    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes('\n'.join(header).encode('ascii'))
    notice_out.write_bytes(notice_text.encode('ascii'))


def main(argv: list[str] | None = None) -> int:
    """Command-line entry point."""
    parser = argparse.ArgumentParser(description=__doc__.split('\n', 1)[0])
    parser.add_argument('--afm-dir', type=Path, required=True)
    parser.add_argument('--jar', type=Path, default=None)
    parser.add_argument('--pdfbox-version', default='3.0.7')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--notice-out', type=Path, required=True)
    args = parser.parse_args(argv)
    build(args.afm_dir, args.jar, args.pdfbox_version, args.out, args.notice_out)
    return 0


if __name__ == '__main__':
    sys.exit(main())
