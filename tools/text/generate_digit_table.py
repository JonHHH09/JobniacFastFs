"""Generate the ``is_text_digit`` range table of jobniac-text (stdlib only).

The ranges come from ``crates/text/tests/golden/str-rules-char-classes.json``, which holds the
``isdigit`` predicate of a Python 3.14.5 interpreter (Unicode 16.0.0). The
table is the Unicode Digit property (Numeric_Type Digit or Decimal), which the general category
does not give (U+00B2 is a digit but not a decimal).

Usage (from the repository root):

    py -I tools/text/generate_digit_table.py --golden crates/text/tests/golden --out crates/text/src/str_rules

The script writes ``digits.rs`` into the ``--out`` folder and refuses to write a table that is
empty or not sorted.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def read_ranges(golden_dir: Path) -> list[tuple[int, int]]:
    """Read the ``isdigit`` inclusive ranges as code point pairs."""
    root = json.loads((golden_dir / 'str-rules-char-classes.json').read_text(encoding='utf-8'))
    ranges: list[tuple[int, int]] = []
    for text in root['predicates']['isdigit']['ranges']:
        start, _, end = text.partition('-')
        ranges.append((int(start, 16), int(end or start, 16)))
    if not ranges:
        raise SystemExit('isdigit ranges are empty')
    if ranges != sorted(ranges) or any(low > high for low, high in ranges):
        raise SystemExit('isdigit ranges are not sorted')
    return ranges


def render(ranges: list[tuple[int, int]]) -> str:
    lines = [
        '//! Unicode Digit property ranges (Unicode 16.0.0) for `jobniac-text`.',
        '//!',
        '//! The Digit property holds for decimal digits (category Nd) and for other characters with a',
        '//! numeric type of Digit, such as superscripts and circled digits, so U+00B2 is a digit but not',
        '//! a decimal. The table is generated; do not edit by hand.',
        '',
        '/// Inclusive `(first, last)` code point ranges for which `str.isdigit()` is true, sorted.',
        'pub static DIGIT_RANGES: &[(u32, u32)] = &[',
    ]
    for low, high in ranges:
        lines.append(f'    (0x{low:04X}, 0x{high:04X}),')
    lines.append('];')
    return '\n'.join(lines) + '\n'


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--golden', required=True, type=Path, help='folder with the golden JSON')
    parser.add_argument('--out', required=True, type=Path, help='folder that receives digits.rs')
    args = parser.parse_args()
    ranges = read_ranges(args.golden)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / 'digits.rs').write_text(render(ranges), encoding='utf-8', newline='\n')
    print(f'wrote {len(ranges)} ranges to {args.out / "digits.rs"}')


if __name__ == '__main__':
    main()
