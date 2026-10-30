#!/usr/bin/env python3
"""Renders docs/demo.svg: an animated terminal showing odjazd's real, coloured output.

Run from the repository root:

    cargo build --release
    python3 docs/demo.py swietokrzyska

The command runs for real with colours forced on. Its ANSI output becomes SVG text, and
CSS animations type the command and then reveal the board. The file embeds no fonts or
scripts, so GitHub shows it in the README.
"""

import html
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "release" / "odjazd"
OUT = ROOT / "docs" / "demo.svg"

FONT = 'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace'
SIZE = 15  # font size
CHAR = 9.05  # width of one character cell
LINE = 23  # line height
PAD = 22  # padding inside the window
BAR = 36  # title bar height

FG, BG, DIM, PROMPT = "#e6edf3", "#0d1117", "#7d8590", "#3fb950"

# Timeline, in seconds.
TYPE_START = 0.7  # typing starts
TYPE_STEP = 0.08  # per typed character
RUN_DELAY = 0.75  # from Enter to the output (about how long a query takes)
LINE_STEP = 0.03  # between output lines
FADE_AT = 11.5  # everything fades out...
CYCLE = 13.0  # ...and the loop starts over

SGR = re.compile(r"\x1b\[([0-9;]*)m")


def runs(line):
    """Splits a line with ANSI colours into (text, style) runs."""
    style, result, pos = {}, [], 0
    for match in SGR.finditer(line):
        if match.start() > pos:
            result.append((line[pos : match.start()], dict(style)))
        apply_sgr(style, match.group(1))
        pos = match.end()
    if pos < len(line):
        result.append((line[pos:], dict(style)))
    return result


def apply_sgr(style, params):
    codes = [int(code) for code in params.split(";") if code] or [0]
    i = 0
    while i < len(codes):
        code = codes[i]
        if code == 0:
            style.clear()
        elif code == 1:
            style["bold"] = True
        elif code == 2:
            style["dim"] = True
        elif code == 7:
            style["invert"] = True
        elif code in (38, 48) and codes[i + 1 : i + 2] == [2]:
            r, g, b = codes[i + 2 : i + 5]
            style["fg" if code == 38 else "bg"] = f"#{r:02x}{g:02x}{b:02x}"
            i += 4
        i += 1


def baseline(row):
    return BAR + PAD + 12 + row * LINE


def draw(line_runs, row, start_col=0):
    """SVG for one line; every word sits exactly in its character cell."""
    parts, col, y = [], start_col, baseline(row)
    for text, style in line_runs:
        fg, bg = style.get("fg", FG), style.get("bg")
        if style.get("invert"):
            fg, bg = bg or BG, fg
        if style.get("dim"):
            fg = DIM
        x = PAD + col * CHAR
        if bg:
            parts.append(
                f'<rect x="{x:.1f}" y="{y - 16}" width="{len(text) * CHAR:.1f}" height="21" rx="3" fill="{bg}"/>'
            )
        weight = ' font-weight="700"' if style.get("bold") else ""
        for word in re.finditer(r"\S+", text):
            parts.append(
                f'<text x="{x + word.start() * CHAR:.1f}" y="{y}" fill="{fg}"{weight}>{html.escape(word.group())}</text>'
            )
        col += len(text)
    return "".join(parts)


def percent(seconds):
    return seconds / CYCLE * 100


def shown_between(name, start, end=FADE_AT, fade=0.5):
    """Keyframes: hidden, then shown from `start`, fading out from `end`."""
    a, b, c = percent(start), percent(end), percent(end + fade)
    hidden = f"0%,{a - 0.01:.3f}%{{opacity:0}}" if start > 0 else ""
    return f"@keyframes {name}{{{hidden}{a:.3f}%,{b:.3f}%{{opacity:1}}{c:.3f}%,100%{{opacity:0}}}}"


def main():
    args = sys.argv[1:] or ["swietokrzyska"]
    env = {**os.environ, "CLICOLOR_FORCE": "1"}
    output = subprocess.run([BINARY, *args], env=env, capture_output=True, text=True, check=True).stdout
    lines = output.rstrip("\n").split("\n")
    command = "odjazd " + " ".join(f'"{arg}"' if " " in arg else arg for arg in args)

    styles, body = [], []
    prompt = [("$ ", {"fg": PROMPT, "bold": True})]

    # The prompt, then the command typed one character at a time.
    styles.append(shown_between("p0", 0))
    body.append(f'<g style="animation:p0 {CYCLE}s linear infinite">{draw(prompt, 0)}</g>')
    for i, char in enumerate(command):
        if char == " ":
            continue
        styles.append(shown_between(f"c{i}", TYPE_START + i * TYPE_STEP))
        body.append(f'<g style="animation:c{i} {CYCLE}s linear infinite">{draw([(char, {})], 0, 2 + i)}</g>')

    # A cursor that follows the typing and disappears on Enter.
    enter = TYPE_START + len(command) * TYPE_STEP + 0.3
    steps = [f"0%{{transform:translateX({2 * CHAR:.1f}px)}}"]
    for i in range(len(command)):
        moment = percent(TYPE_START + i * TYPE_STEP)
        steps.append(f"{moment:.3f}%{{transform:translateX({(3 + i) * CHAR:.1f}px)}}")
    styles.append(f"@keyframes cursor{{{''.join(steps)}}}")
    styles.append(shown_between("typing", 0, enter, fade=0.01))
    cursor = f'<rect x="{PAD}" y="{baseline(0) - 14}" width="{CHAR:.1f}" height="18" fill="{FG}" opacity="0.85"/>'
    body.append(
        f'<g class="typing" style="animation:typing {CYCLE}s linear infinite">'
        f'<g style="animation:cursor {CYCLE}s steps(1,end) infinite">{cursor}</g></g>'
    )

    # The board, printed line by line.
    shown = enter + RUN_DELAY
    for row, line in enumerate(lines, start=1):
        if not SGR.sub("", line).strip():
            continue
        styles.append(shown_between(f"l{row}", shown + row * LINE_STEP))
        body.append(f'<g style="animation:l{row} {CYCLE}s linear infinite">{draw(runs(line), row)}</g>')

    # A fresh prompt with a blinking cursor.
    last = len(lines) + 1
    styles.append(shown_between("done", shown + last * LINE_STEP + 0.2))
    styles.append("@keyframes blink{0%,50%{opacity:1}50.01%,100%{opacity:0}}")
    cursor = f'<rect x="{PAD + 2 * CHAR:.1f}" y="{baseline(last) - 14}" width="{CHAR:.1f}" height="18" fill="{FG}" opacity="0.85"/>'
    body.append(
        f'<g style="animation:done {CYCLE}s linear infinite">{draw(prompt, last)}'
        f'<g style="animation:blink 1s linear infinite">{cursor}</g></g>'
    )

    columns = max(len(SGR.sub("", line)) for line in [*lines, "$ " + command])
    width = round(2 * PAD + columns * CHAR)
    height = baseline(last) + PAD
    # Without motion, show the finished board and no typing cursor.
    styles.append("@media (prefers-reduced-motion:reduce){*{animation:none!important}.typing{display:none}}")
    svg = f"""<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{html.escape(command)}: next departures, with each line in its own colours">
<title>{html.escape(command)}</title>
<style>{"".join(styles)}</style>
<rect x="0.5" y="0.5" width="{width - 1}" height="{height - 1}" rx="10" fill="{BG}" stroke="#30363d"/>
<path d="M10.5 0.5h{width - 21}a10 10 0 0 1 10 10v{BAR - 10.5}h-{width - 1}v-{BAR - 10.5}a10 10 0 0 1 10-10z" fill="#161b22"/>
<line x1="0.5" y1="{BAR}" x2="{width - 0.5}" y2="{BAR}" stroke="#30363d"/>
<circle cx="20" cy="{BAR / 2}" r="6" fill="#ff5f57"/><circle cx="40" cy="{BAR / 2}" r="6" fill="#febc2e"/><circle cx="60" cy="{BAR / 2}" r="6" fill="#28c840"/>
<text x="{width / 2}" y="{BAR / 2 + 4.5}" fill="{DIM}" font-family="system-ui, -apple-system, 'Segoe UI', sans-serif" font-size="13" text-anchor="middle">odjazd</text>
<g font-family='{FONT}' font-size="{SIZE}">
{chr(10).join(body)}
</g>
</svg>
"""
    OUT.write_text(svg, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)} ({width}×{height}, {len(lines)} lines)")


if __name__ == "__main__":
    main()
