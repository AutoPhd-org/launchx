"""Generate the AutoPhD story figures as SVG.

    python3 docs/figures.py github <out-dir> [name ...]   theme-aware files (they follow the reader's light or dark mode)
    python3 docs/figures.py blog <out-dir> [name ...]     one light and one dark file per figure, in the blog's colours

github writes bottleneck.svg, direct-state.svg, shared-interface.svg and ecosystem.svg.
blog writes pipeline-then, pipeline-now, direct-state and shared-interface, each as
<name>.svg and <name>-dark.svg. Names after the directory limit the output to those figures.

Edit this script, not the SVG files.
"""
import sys
from pathlib import Path
from xml.sax.saxutils import escape

FONT = 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif'
MONO = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'
HAND = '"Bradley Hand", "Segoe Print", "Comic Sans MS", cursive'

# Colour tokens. `on` is the text colour on a solid accent fill.
PALETTES = {
    "github": {
        "light": dict(
            bg="#f7f8fb", frame="#e6e9f0", card="#ffffff", cardb="#dfe3eb", panel="#eef1f6", panelb="#dfe3eb",
            ink="#15171c", ink2="#353a45", muted="#5b6271", faint="#8a93a6", line="#aab2c2",
            you="#3b5bdb", yous="#edf1ff", youb="#b9c6f5",
            agent="#6741d9", agents="#f1edff", agentb="#cbbdf5",
            app="#2b8a3e", apps="#eaf6ec", appb="#a9d9b3",
            hot="#c92a2a", hots="#fdecec", hotb="#f1b0b0",
            pen="#c25d00", on="#ffffff"),
        "dark": dict(
            bg="#0f1115", frame="#22262f", card="#171a21", cardb="#2b303b", panel="#12151b", panelb="#2b303b",
            ink="#e8eaef", ink2="#c3c8d2", muted="#9aa1ad", faint="#6b7385", line="#4a5162",
            you="#8fa5ff", yous="#161d38", youb="#34417a",
            agent="#b197fc", agents="#1d1838", agentb="#47397f",
            app="#63d47a", apps="#11261a", appb="#2a6b3a",
            hot="#ff8787", hots="#2d1516", hotb="#7d3436",
            pen="#ffa94d", on="#0f1115"),
    },
    "blog": {
        "light": dict(
            bg="#ffffff", frame="#e5e1d8", card="#ffffff", cardb="#d9d4c8", panel="#f6f4ef", panelb="#e5e1d8",
            ink="#1b1a18", ink2="#45423d", muted="#6b675f", faint="#99948a", line="#b9b3a6",
            you="#23508e", yous="#e7eef8", youb="#a9c1e4",
            agent="#6a45c0", agents="#f3effb", agentb="#c9b8ee",
            app="#1d7a4b", apps="#ebf6ef", appb="#a5d6ba",
            hot="#b42318", hots="#fdeeec", hotb="#f0b4ad",
            pen="#b0501d", on="#ffffff"),
        "dark": dict(
            bg="#171a20", frame="#2a2e36", card="#1d2128", cardb="#3b404b", panel="#13161b", panelb="#2a2e36",
            ink="#ebe9e4", ink2="#c8c5be", muted="#9d9b96", faint="#73726e", line="#565b66",
            you="#8fb8ff", yous="#1a2538", youb="#34507f",
            agent="#b89ff3", agents="#1e192d", agentb="#4b3d78",
            app="#6fd09f", apps="#11241a", appb="#2c6a4a",
            hot="#f4877b", hots="#2b1614", hotb="#7a342d",
            pen="#f0a46c", on="#101216"),
    },
}
ARROWS = ("line", "faint", "muted", "you", "agent", "app", "hot")


def rules(tokens):
    return " ".join(f".f-{k}{{fill:{v}}} .s-{k}{{stroke:{v}}}" for k, v in tokens.items())


def tw(s, size, weight=400, mono=False):
    """Rough rendered width of a string, enough to size pills and place neighbours."""
    if mono:
        return len(s) * size * 0.602
    w = 0.0
    for ch in s:
        if ch == " ":
            w += 0.27
        elif ch in "il.,:;'|!·":
            w += 0.25
        elif ch in "jtfrI()[]/-":
            w += 0.35
        elif ch in "mw":
            w += 0.83
        elif ch in "MW":
            w += 0.9
        elif ch.isupper():
            w += 0.66
        elif ch.isdigit():
            w += 0.6
        else:
            w += 0.55
    return w * size * (1.05 if weight >= 650 else 1.0)


class Svg:
    """A drawing whose colours come from palette tokens (classes f-<token>, s-<token>)."""

    def __init__(self, w, h, title, desc, palette, mode):
        self.w, self.h, self.title, self.desc = w, h, title, desc
        self.palette, self.mode = PALETTES[palette], mode
        self.parts = []

    def add(self, s):
        self.parts.append(s)

    @staticmethod
    def _paint(fill, stroke, sw, dash, extra=""):
        cls = " ".join(c for c in (fill and f"f-{fill}", stroke and f"s-{stroke}") if c)
        out = f' class="{cls}"' if cls else ""
        if not fill:
            out += ' fill="none"'
        if stroke:
            out += f' stroke-width="{sw}"'
            if dash:
                out += f' stroke-dasharray="{dash}"'
        return out + extra

    def rect(self, x, y, w, h, r=0, fill=None, stroke=None, sw=1.5, dash=None):
        self.add(f'<rect x="{x:g}" y="{y:g}" width="{w:g}" height="{h:g}" rx="{r:g}"{self._paint(fill, stroke, sw, dash)}/>')

    def circle(self, cx, cy, r, fill=None, stroke=None, sw=1.5, dash=None):
        self.add(f'<circle cx="{cx:g}" cy="{cy:g}" r="{r:g}"{self._paint(fill, stroke, sw, dash)}/>')

    def path(self, d, fill=None, stroke=None, sw=1.5, dash=None, end=False, start=False, extra=""):
        more = ' stroke-linecap="round" stroke-linejoin="round"' if stroke else ""
        if end:
            more += f' marker-end="url(#ah-{stroke})"'
        if start:
            more += f' marker-start="url(#ah-{stroke})"'
        self.add(f'<path d="{d}"{self._paint(fill, stroke, sw, dash, more + extra)}/>')

    def line(self, x1, y1, x2, y2, stroke="line", sw=1.5, dash=None, end=False, start=False):
        self.path(f"M{x1:g},{y1:g} L{x2:g},{y2:g}", stroke=stroke, sw=sw, dash=dash, end=end, start=start)

    def text(self, x, y, s, fill="ink", size=14, weight=400, anchor="start", mono=False, spacing=None,
             italic=False, family=None, extra=""):
        a = f' font-size="{size:g}"'
        if weight != 400:
            a += f' font-weight="{weight}"'
        if anchor != "start":
            a += f' text-anchor="{anchor}"'
        if mono or family:
            a += f" font-family='{family or MONO}'"
        if spacing:
            a += f' letter-spacing="{spacing}"'
        if italic:
            a += ' font-style="italic"'
        self.add(f'<text x="{x:g}" y="{y:g}" class="f-{fill}"{a}{extra}>{escape(s)}</text>')

    def duo(self, x, y, strong, rest, size=14, anchor="start", strong_fill="ink", rest_fill="muted"):
        """A bold word followed by lighter text, as one line."""
        a = f' text-anchor="{anchor}"' if anchor != "start" else ""
        self.add(f'<text x="{x:g}" y="{y:g}" font-size="{size:g}"{a}><tspan class="f-{strong_fill}" font-weight="700">'
                 f'{escape(strong)}</tspan><tspan class="f-{rest_fill}"> {escape(rest)}</tspan></text>')

    def pill(self, x, y, label, fill, text_fill, size=11, h=22, weight=800, spacing="1.1", stroke=None, pad=10, mono=False):
        w = round(tw(label, size, weight, mono) + (len(label) - 1) * float(spacing or 0) + pad * 2)
        self.rect(x, y, w, h, h / 2, fill=fill, stroke=stroke, sw=1.2)
        self.text(x + w / 2, y + h / 2 + size * 0.36, label, fill=text_fill, size=size, weight=weight,
                  anchor="middle", spacing=spacing, mono=mono)
        return w

    def render(self):
        p = self.palette
        if self.mode == "auto":
            style = rules(p["light"]) + " @media (prefers-color-scheme: dark) { " + rules(p["dark"]) + " }"
        else:
            style = rules(p[self.mode])
        markers = "".join(
            f'<marker id="ah-{k}" viewBox="0 0 10 10" refX="8.5" refY="5" markerWidth="6.5" markerHeight="6.5" '
            f'orient="auto-start-reverse"><path d="M0,0.6 L10,5 L0,9.4 z" class="f-{k}"/></marker>' for k in ARROWS)
        return (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" width="{self.w}" height="{self.h}" '
            f"role=\"img\" aria-labelledby=\"title desc\" font-family='{FONT}'>\n"
            f'<title id="title">{escape(self.title)}</title>\n<desc id="desc">{escape(self.desc)}</desc>\n'
            f"<style>{style}</style>\n<defs>{markers}</defs>\n"
            f'<rect x="1" y="1" width="{self.w - 2}" height="{self.h - 2}" rx="24" class="f-bg s-frame" stroke-width="1.5"/>\n'
            + "\n".join(self.parts) + "\n</svg>\n")


# ---------------------------------------------------------------- small drawings

def heading(s, eyebrow, title, y=0):
    s.text(48, y + 54, eyebrow, fill="you", size=12.5, weight=800, spacing="2.2")
    s.text(48, y + 91, title, fill="ink", size=27, weight=750, spacing="-0.4")


def person(s, cx, top, color="you", k=1.0):
    s.circle(cx, top + 13 * k, 13 * k, fill=color)
    s.path(f"M{cx - 23 * k:g},{top + 58 * k:g} C{cx - 23 * k:g},{top + 39 * k:g} {cx - 13 * k:g},{top + 31 * k:g} {cx:g},{top + 31 * k:g} "
           f"C{cx + 13 * k:g},{top + 31 * k:g} {cx + 23 * k:g},{top + 39 * k:g} {cx + 23 * k:g},{top + 58 * k:g} Z", fill=color)


def sparkle(s, cx, cy, r, color="agent"):
    s.path(f"M{cx:g},{cy - r:g} Q{cx:g},{cy:g} {cx + r:g},{cy:g} Q{cx:g},{cy:g} {cx:g},{cy + r:g} "
           f"Q{cx:g},{cy:g} {cx - r:g},{cy:g} Q{cx:g},{cy:g} {cx:g},{cy - r:g} Z", fill=color)


def window_icon(s, x, y, w, h, color="app"):
    s.rect(x, y, w, h, 7, stroke=color, sw=2.4)
    s.line(x, y + 13, x + w, y + 13, stroke=color, sw=2.4)
    for i in range(3):
        s.circle(x + 9 + i * 8, y + 6.8, 2, fill=color)
    s.rect(x + 9, y + 21, w * 0.34, 5, 2.5, fill=color)
    s.rect(x + 9, y + 31, w * 0.56, 5, 2.5, fill=color)


def cursor(s, x, y, color, k=1.0):
    pts = [(0, 0), (0, 16), (4.4, 12.2), (7.4, 18.6), (10, 17.4), (7.1, 11.2), (12.6, 11.2)]
    d = "M" + " L".join(f"{x + px * k:g},{y + py * k:g}" for px, py in pts) + " Z"
    s.path(d, fill=color, stroke="bg", sw=1.4)


def check(s, x, y, color, k=1.0, sw=2.2):
    s.path(f"M{x:g},{y + 4.5 * k:g} L{x + 3.6 * k:g},{y + 8 * k:g} L{x + 10 * k:g},{y:g}", stroke=color, sw=sw)


def lock(s, x, y, color):
    s.rect(x, y + 5, 11, 8.5, 2, fill=color)
    s.path(f"M{x + 2.5:g},{y + 5:g} V{y + 3.4:g} a3,3 0 0 1 6,0 V{y + 5:g}", stroke=color, sw=1.7)


def toggle(s, cx, cy, on):
    s.rect(cx - 15, cy - 8.5, 30, 17, 8.5, fill="app" if on else "line")
    s.circle(cx + (6.5 if on else -6.5), cy, 5.8, fill="on" if on else "card")


def act_chip(s, cx, cy, value):
    style = {"yes": ("apps", "app", "appb"), "ask": ("agents", "agent", "agentb"), "no": ("panel", "faint", "panelb")}[value]
    s.rect(cx - 19, cy - 10, 38, 20, 10, fill=style[0], stroke=style[2], sw=1.2)
    s.text(cx, cy + 4.3, value, fill=style[1], size=12, weight=700, anchor="middle")


ACCESS = [("tasks", "Tasks", True, "ask"), ("today.plan", "Today's plan", True, "yes"),
          ("deadlines", "Deadlines", True, "no"), ("journal", "Journal", False, "no")]


def access_rows(s, x, first, pitch, see_x, act_x, right, mono):
    """The see / act switches, one row per part of the state."""
    s.text(see_x, first - pitch + 4, "see", fill="muted", size=11.5, weight=700, anchor="middle", spacing="0.4")
    s.text(act_x, first - pitch + 4, "act", fill="muted", size=11.5, weight=700, anchor="middle", spacing="0.4")
    for i, (key, name, see, act) in enumerate(ACCESS):
        y = first + i * pitch
        s.line(x, y - pitch / 2 - 4.5, right, y - pitch / 2 - 4.5, stroke="panelb", sw=1)
        s.text(x, y, key if mono else name, fill="ink2" if see else "faint", size=13 if mono else 13.5, mono=mono)
        toggle(s, see_x, y - 4.5, see)
        act_chip(s, act_x, y - 4.5, act)


# ---------------------------------------------------------------- 1 and 2: where the bottleneck is

def lane(s, xa, xb, yc, right, hot, flare=0):
    """One direction of a link, drawn as a pipe with an arrowhead. `flare` (-1 up, +1 down) widens the
    source end into a funnel: more waits to get in than fits through."""
    t, head = 9, 15
    fill, edge, ink = ("hots", "hot", "hot") if hot else ("panel", "line", "faint")
    src, tip = (xa, xb) if right else (xb, xa)
    sign = 1 if right else -1
    neck = tip - sign * head
    if flare:
        fy = yc + flare * (t + 36)
        s.path(f"M{src:g},{yc - flare * t:g} L{src:g},{fy:g} C{src + sign * 20:g},{fy:g} {src + sign * 22:g},{yc + flare * t:g} "
               f"{src + sign * 48:g},{yc + flare * t:g} L{neck:g},{yc + flare * t:g} L{neck:g},{yc - flare * t:g} Z",
               fill=fill, stroke=edge, sw=1.6)
        for dx, dy in ((9, 3), (9, 16), (9, 29), (9, 41), (20, 6), (21, 19), (31, 3), (33, 13), (44, 1)):
            s.circle(src + sign * dx, yc + flare * (dy - 4), 3.4, fill=ink)
        for dx in (70, 104):
            if abs(neck - src) > dx + 30:
                s.circle(src + sign * dx, yc, 3.4, fill=ink)
    else:
        left = min(src, neck)
        s.rect(left, yc - t, abs(neck - src), t * 2, t, fill=fill, stroke=edge, sw=1.6)
        n = 3
        for i in range(n):
            s.circle(src + sign * (abs(neck - src) * (i + 1) / (n + 1)), yc, 3.2, fill=ink)
    s.path(f"M{neck:g},{yc - 14:g} L{tip:g},{yc:g} L{neck:g},{yc + 14:g} Z", fill=ink)


def callout(s, x, y, w, h, notch, tag, head, lines):
    r = 14
    s.path(f"M{x + r:g},{y:g} H{notch - 11:g} L{notch:g},{y - 12:g} L{notch + 11:g},{y:g} H{x + w - r:g} "
           f"A{r},{r} 0 0 1 {x + w:g},{y + r:g} V{y + h - r:g} A{r},{r} 0 0 1 {x + w - r:g},{y + h:g} H{x + r:g} "
           f"A{r},{r} 0 0 1 {x:g},{y + h - r:g} V{y + r:g} A{r},{r} 0 0 1 {x + r:g},{y:g} Z", fill="hots", stroke="hot", sw=1.8)
    pw = s.pill(x + 18, y + 16, "BOTTLENECK", "hot", "on")
    s.text(x + 18 + pw + 10, y + 31.5, tag, fill="hot", size=11.5, weight=800, spacing="1.1")
    s.text(x + 18, y + 67, head, fill="ink", size=17.5, weight=700)
    for i, ln in enumerate(lines):
        s.text(x + 18, y + 91 + i * 20, ln, fill="ink2", size=14)


def pipeline(s, oy, era):
    now = era == "now"
    heading(s, "2026 · NOW" if now else "2023 · THEN",
            "Now the communication is the bottleneck" if now else "The agent was the bottleneck", oy)
    s.rect(1050, oy + 41, 26, 16, 8, fill="hots", stroke="hot", sw=1.6)
    s.text(1084, oy + 54, "bottleneck", fill="muted", size=13.5)

    cy = oy + 252
    # You and the software sit in the same place in both pictures.
    s.rect(48, cy - 78, 176, 156, 20, fill="yous", stroke="youb")
    person(s, 136, cy - 60, "you")
    s.text(136, cy + 32, "You", size=22, weight=700, anchor="middle")
    s.text(136, cy + 55, "see, think, decide", fill="muted", size=13.5, anchor="middle")
    s.rect(976, cy - 78, 176, 156, 20, fill="apps", stroke="appb")
    window_icon(s, 1034, cy - 58, 60, 46, "app")
    s.text(1064, cy + 32, "Software", size=22, weight=700, anchor="middle")
    s.text(1064, cy + 55, "state behind a GUI", fill="muted", size=13.5, anchor="middle")

    outer = 116 if now else 52
    a1, a2 = 236, 600 - outer - 12       # you <-> agent
    b1, b2 = 600 + outer + 12, 964       # agent <-> software
    top, bottom = cy - 17, cy + 17
    lane(s, a1, a2, top, True, now, flare=-1 if now else 0)
    lane(s, a1, a2, bottom, False, now, flare=1 if now else 0)
    lane(s, b1, b2, top, True, now, flare=-1 if now else 0)
    lane(s, b1, b2, bottom, False, now, flare=1 if now else 0)
    up, down = cy - 37, cy + 48
    if now:
        s.duo(a1 + 56, up, "write", "everything, in words")
        s.duo(a2 - 56, down, "read", "long answers", anchor="end")
        s.duo(b1 + 56, up, "executes", "click by click")
        s.duo(b2 - 56, down, "explores", "screenshots, files", anchor="end")
    else:
        s.duo((a1 + a2) / 2, up, "write", "a prompt", anchor="middle")
        s.duo((a1 + a2) / 2, down, "read", "an answer", anchor="middle")
        s.duo((b1 + b2) / 2, up, "executes", "a tool call or two", anchor="middle")
        s.duo((b1 + b2) / 2, down, "explores", "what the tools return", anchor="middle")

    if now:
        s.circle(600, cy, 111, stroke="agentb", sw=9)
        s.circle(600, cy, 100, fill="agents", stroke="agent", sw=2.2)
        s.pill(600 - 42, cy - 122, "HARNESS", "agent", "on", size=10.5, h=21)
        sparkle(s, 600, cy - 50, 15)
        s.text(600, cy - 4, "Agent", size=28, weight=750, anchor="middle")
        s.text(600, cy + 22, "plans, calls tools,", fill="ink2", size=14.5, anchor="middle")
        s.text(600, cy + 42, "works for hours", fill="ink2", size=14.5, anchor="middle")
        callout(s, 48, cy + 110, 424, 126, (a1 + a2) / 2, "YOU ↔ AGENT", "What you see has to become words",
                ["You describe your screen in sentences, then dig", "through a long reply for the one line you need."])
        callout(s, 728, cy + 110, 424, 126, (b1 + b2) / 2, "AGENT ↔ SOFTWARE", "The agent rebuilds the app's state",
                ["Screenshots, clicks and file reads burn tokens", "before the work you asked for even starts."])
        s.text(600, cy + 158, "models and harnesses", fill="faint", size=13.5, anchor="middle", italic=True)
        s.text(600, cy + 177, "keep getting better", fill="faint", size=13.5, anchor="middle", italic=True)
    else:
        s.circle(600, cy, 52, stroke="hot", sw=1.8, dash="3 6")
        s.circle(600, cy, 38, fill="hots", stroke="hot", sw=2.2)
        s.text(600, cy + 5, "Agent", size=15, weight=700, anchor="middle")
        callout(s, 378, cy + 110, 444, 126, 600, "THE AGENT", "The agent wasn't smart enough",
                ["It lost the plan, repeated itself and stopped too early.", "A chat box and a few tools were all it could use."])
        for x in ((a1 + a2) / 2, (b1 + b2) / 2):
            s.text(x, cy + 76, "wide enough, for now", fill="faint", size=13.5, anchor="middle", italic=True)


PIPE_H = 520
THEN_DESC = ("2023: you write prompts and read answers; the agent executes a few tool calls on software and explores "
             "what they return. The agent is the bottleneck: it loses the plan, repeats itself and stops too early.")
NOW_DESC = ("2026: the agent, wrapped in a harness, plans, calls tools and works for hours. The two links are now the "
            "bottleneck. Between you and the agent, everything you see has to become words and you read long answers. "
            "Between the agent and the software, screenshots, clicks and file reads burn tokens before the work starts.")


def fig_pipeline(palette, mode, era):
    s = Svg(1200, PIPE_H, "The agent was the bottleneck" if era == "then" else "Now the communication is the bottleneck",
            THEN_DESC if era == "then" else NOW_DESC, palette, mode)
    pipeline(s, 0, era)
    return s.render()


def fig_bottleneck(palette, mode):
    gap = 64
    s = Svg(1200, PIPE_H * 2 + gap, "The bottleneck moved", THEN_DESC + " " + NOW_DESC, palette, mode)
    pipeline(s, 0, "then")
    y = PIPE_H + gap / 2
    s.line(48, y, 1152, y, stroke="panelb", sw=1.5, dash="2 7")
    label = "three years of better models and better harnesses"
    w = round(tw(label, 13.5, 600) + 96)
    s.rect(600 - w / 2, y - 16, w, 32, 16, fill="card", stroke="cardb", sw=1.3)
    s.text(600 - w / 2 + 22, y + 4.8, label, fill="muted", size=13.5, weight=600)
    s.path(f"M{600 + w / 2 - 24:g},{y - 6:g} v11 m-4.5,-4.5 l4.5,4.5 l4.5,-4.5", stroke="muted", sw=1.8)
    pipeline(s, PIPE_H + gap, "now")
    return s.render()


# ---------------------------------------------------------------- 3: the agent works on the state

def fig_direct_state(palette, mode):
    s = Svg(1200, 668, "The agent works on the app's state, not on its pixels",
            "An app has one state document and a page that is one view of it. You use the page. Your agent reads the "
            "state and runs commands directly, through access switches you set for each part of the state: what it may "
            "see and whether it may act, ask first, or not act. The old route, where the agent reaches the state through "
            "screenshots and clicks on the page, is shown as a long detour.", palette, mode)
    heading(s, "WHERE THIS IS HEADING · 1", "The agent works on the app's state, not on its pixels")

    # The long way round: through the page, one screenshot at a time.
    s.path("M148,456 V170 Q148,152 166,152 H992 Q1010,152 1010,170 V212", stroke="faint", sw=1.8, dash="5 6", end=True)
    s.text(176, 138, "Today: the long way round. Every step is a screenshot and another model call.", fill="muted", size=13.5)
    for i in range(4):
        x = 700 + i * 60
        s.rect(x - 5, 141, 50, 22, 0, fill="bg")
        s.rect(x, 144, 22, 16, 3.5, fill="panel", stroke="faint", sw=1.4)
        s.line(x, 149, x + 22, 149, stroke="faint", sw=1.2)
        cursor(s, x + 28, 145, "faint", 0.82)

    # The app: a page on top of one state document.
    s.rect(600, 178, 552, 462, 22, fill="panel", stroke="panelb")
    s.text(624, 204, "ONE APP", fill="muted", size=11.5, weight=800, spacing="1.6")
    s.rect(624, 216, 504, 142, 16, fill="card", stroke="cardb")
    s.text(646, 252, "The page", size=18.5, weight=700)
    s.text(646, 275, "what you look at", fill="muted", size=13.5)
    s.text(646, 293, "and touch", fill="muted", size=13.5)
    s.pill(646, 312, "one view of the state", "yous", "you", size=11.5, h=24, weight=700, spacing="0.2", stroke="youb")
    for c, n in enumerate((3, 2, 2)):
        cx = 836 + c * 94
        s.rect(cx, 236, 38, 6, 3, fill="line")
        for r in range(n):
            y = 252 + r * 30
            if c == 0 and r == 1:
                s.rect(cx, y, 80, 22, 6, fill="agents", stroke="agent", sw=1.4, dash="4 3")
            else:
                s.rect(cx, y, 80, 22, 6, fill="panel", stroke="panelb", sw=1.3)
                s.rect(cx + 9, y + 8.5, 34 + (r * 13 + c * 7) % 24, 5, 2.5, fill="line")
    s.line(876, 362, 876, 394, stroke="muted", sw=1.8, end=True, start=True)
    s.text(890, 383, "the same data, both ways", fill="muted", size=12.5)

    s.rect(624, 398, 504, 218, 16, fill="card", stroke="app", sw=2.2)
    s.text(646, 434, "The state", size=18.5, weight=700)
    s.text(646, 457, "one JSON document,", fill="muted", size=13.5)
    s.text(646, 475, "one command per action", fill="muted", size=13.5)
    s.pill(646, 496, "GET /api/state", "apps", "app", size=12, h=25, weight=600, spacing="0", stroke="appb", mono=True)
    s.pill(646, 529, "$ app <command>", "apps", "app", size=12, h=25, weight=600, spacing="0", stroke="appb", mono=True)
    s.rect(836, 414, 274, 186, 12, fill="panel", stroke="panelb", sw=1.3)
    rows = [("tasks", "9 open", True), ("today.plan", "3 blocks", True), ("deadlines", "5", True),
            ("habits", "4", True), ("journal", "not shared", False)]
    for i, (key, val, see) in enumerate(rows):
        y = 446 + i * 31
        if see:
            s.circle(856, y - 4.5, 4.5, fill="app")
        else:
            lock(s, 850.5, y - 13, "faint")
        s.text(872, y, key, fill="ink" if see else "faint", size=13.5, mono=True)
        s.text(1094, y, val, fill="muted" if see else "faint", size=13, mono=True, anchor="end")

    # You: the page, and the switches.
    s.rect(300, 236, 220, 96, 18, fill="yous", stroke="youb")
    person(s, 344, 256, "you", 0.86)
    s.text(380, 280, "You", size=20, weight=700)
    s.text(380, 302, "look, click, drag", fill="muted", size=13.5)
    s.line(522, 284, 620, 284, stroke="you", sw=2.2, end=True)
    s.line(410, 334, 410, 394, stroke="you", sw=2.2, end=True)
    s.text(424, 370, "you decide", fill="you", size=13, weight=600)

    s.rect(300, 398, 220, 218, 18, fill="card", stroke="you", sw=2.2)
    s.text(318, 426, "AGENT ACCESS", fill="you", size=11.5, weight=800, spacing="1.4")
    access_rows(s, 318, 492, 32, 438, 486, 506, mono=True)

    # The agent: straight to the state, through the switches.
    s.rect(48, 456, 200, 96, 18, fill="agents", stroke="agentb")
    s.circle(88, 504, 19, fill="card", stroke="agent", sw=2)
    sparkle(s, 88, 504, 11)
    s.text(118, 499, "Your agent", size=18, weight=700)
    s.text(118, 520, "any harness", fill="muted", size=13.5)
    s.line(251, 504, 297, 504, stroke="agent", sw=2.2, end=True, start=True)
    s.line(620, 478, 524, 478, stroke="app", sw=2.2, end=True)
    s.text(572, 468, "reads", fill="app", size=13, weight=700, anchor="middle")
    s.line(524, 530, 620, 530, stroke="app", sw=2.2, end=True)
    s.text(572, 551, "acts", fill="app", size=13, weight=700, anchor="middle")
    return s.render()


# ---------------------------------------------------------------- 4: one interface for both

def badge(s, cx, cy, n):
    s.circle(cx, cy, 11.5, fill="ink", stroke="bg", sw=2)
    s.text(cx, cy + 4.6, str(n), fill="bg", size=12.5, weight=800, anchor="middle")


def task(s, x, y, title, meta, w=228, h=56, done=False):
    s.rect(x, y, w, h, 10, fill="card", stroke="cardb", sw=1.3)
    if done:
        s.circle(x + 20, y + 28, 9, fill="apps", stroke="appb", sw=1.2)
        check(s, x + 15, y + 24.5, "app", 1.0, 2)
        s.text(x + 38, y + 24, title, fill="faint", size=13.5, weight=600, extra=' text-decoration="line-through"')
        s.text(x + 38, y + 42, meta, fill="faint", size=12)
    else:
        s.text(x + 14, y + 24, title, fill="ink", size=13.5, weight=600)
        s.text(x + 14, y + 42, meta, fill="muted", size=12)


def keycap(s, x, y, key, label):
    s.rect(x, y + 2, 26, 24, 6, fill="line")
    s.rect(x, y, 26, 24, 6, fill="card", stroke="line", sw=1.2)
    s.text(x + 13, y + 16.5, key, fill="ink", size=12.5, weight=700, anchor="middle", mono=True)
    s.text(x + 34, y + 17, label, fill="muted", size=13)
    return x + 34 + tw(label, 13) + 26


def fig_shared_interface(palette, mode):
    s = Svg(1200, 660, "You and the agent share one interface",
            "One app window that you and your agent both use. A task board shows a task the agent suggested, with Add and "
            "Reject buttons; a task you circled with a pen and wrote a note on; and a task the agent just added, with "
            "undo. Shortcut keys accept, reject, undo and ask. A side panel has switches for what the agent can see and "
            "do in each part of the app, and the chat box is still there.", palette, mode)
    heading(s, "WHERE THIS IS HEADING · 2", "You and the agent share one interface")

    wx, wy, ww, wh = 48, 120, 1104, 462
    s.rect(wx, wy, ww, wh, 18, fill="card", stroke="cardb", sw=1.6)
    s.path(f"M{wx},{wy + 42} V{wy + 18} A18,18 0 0 1 {wx + 18},{wy} H{wx + ww - 18} A18,18 0 0 1 {wx + ww},{wy + 18} V{wy + 42} Z", fill="panel")
    s.line(wx, wy + 42, wx + ww, wy + 42, stroke="cardb", sw=1.3)
    for i in range(3):
        s.circle(wx + 24 + i * 18, wy + 21, 5.5, fill="line")
    s.rect(wx + 92, wy + 9, 236, 24, 12, fill="card", stroke="panelb", sw=1.2)
    s.text(wx + 108, wy + 25.5, "tasks.localhost:7600", fill="muted", size=12.5, mono=True)
    s.text(1004, wy + 25.5, "here now", fill="faint", size=12, anchor="end")
    for x, name, color, soft, edge in ((1014, "You", "you", "yous", "youb"), (1074, "Agent", "agent", "agents", "agentb")):
        w = 52 if name == "You" else 66
        s.rect(x, wy + 9, w, 24, 12, fill=soft, stroke=edge, sw=1.2)
        s.circle(x + 13, wy + 21, 4, fill=color)
        s.text(x + 22, wy + 25.5, name, fill="ink2", size=12.5, weight=600)
    s.line(812, wy + 42, 812, wy + wh, stroke="cardb", sw=1.3)

    # The board.
    cols = (70, 318, 566)
    for x, name, n in zip(cols, ("TO DO", "TODAY", "DONE"), ("2", "3", "2")):
        s.text(x + 2, 192, name, fill="muted", size=11.5, weight=800, spacing="1.3")
        s.text(x + 226, 192, n, fill="faint", size=12, weight=600, anchor="end")
    task(s, cols[0], 206, "Rerun the OSWorld ablation", "importance 9")
    # A suggestion from the agent: answer with a button.
    sx, sy = cols[0], 274
    s.rect(sx, sy, 228, 98, 10, fill="agents", stroke="agent", sw=1.5, dash="5 4")
    sparkle(s, sx + 21, sy + 20, 7)
    s.text(sx + 34, sy + 24, "suggested by your agent", fill="agent", size=11.5, weight=700)
    s.text(sx + 14, sy + 47, "Reply to reviewer 2", fill="ink", size=13.5, weight=600)
    s.rect(sx + 14, sy + 59, 94, 28, 8, fill="app")
    check(s, sx + 31, sy + 69, "on", 0.95, 2.2)
    s.text(sx + 48, sy + 77.5, "Add", fill="on", size=13, weight=700)
    s.rect(sx + 116, sy + 59, 98, 28, 8, fill="card", stroke="cardb", sw=1.3)
    s.path(f"M{sx + 133},{sy + 68.5} l8,8 m0,-8 l-8,8", stroke="muted", sw=2)
    s.text(sx + 150, sy + 77.5, "Reject", fill="ink2", size=13, weight=700)
    cursor(s, sx + 88, sy + 76, "you", 1.05)
    badge(s, sx + 222, sy + 2, 2)
    task(s, cols[0], 384, "Book conference flights", "importance 4")

    task(s, cols[1], 206, "Leg day", "19:30 · 50 min")
    # A task circled with a pen.
    px, py = cols[1], 274
    task(s, px, py, "Draft the related work", "14:00 · 50 min")
    cx, cy = px + 114, py + 28
    s.path(f"M{cx - 124},{cy - 6} C{cx - 122},{cy - 46} {cx + 54},{cy - 50} {cx + 122},{cy - 20} "
           f"C{cx + 152},{cy + 6} {cx + 96},{cy + 44} {cx - 6},{cy + 43} C{cx - 104},{cy + 42} {cx - 150},{cy + 16} {cx - 116},{cy - 20} "
           f"C{cx - 106},{cy - 30} {cx - 92},{cy - 36} {cx - 78},{cy - 39}", stroke="pen", sw=3)
    s.text(px + 20, py + 94, "split this in two", fill="pen", size=19, weight=600, family=HAND,
           extra=f' transform="rotate(-4 {px + 20} {py + 94})"')
    k = (px + 184, py + 70)   # a small pen
    s.path(f"M{k[0]},{k[1] + 22} l4,-11 l15,-15 l7,7 l-15,15 Z", fill="pen")
    s.path(f"M{k[0] + 21},{k[1] - 6} l4,-4 l7,7 l-4,4 Z", fill="pen")
    badge(s, px + 232, py - 10, 3)
    # A task the agent added, where you can see it and undo it.
    ax, ay = cols[1], 394
    s.rect(ax, ay, 228, 56, 10, fill="card", stroke="agentb", sw=1.5)
    s.rect(ax, ay + 10, 4, 36, 2, fill="agent")
    s.text(ax + 16, ay + 24, "Slides for group meeting", fill="ink", size=13.5, weight=600)
    s.add(f'<text x="{ax + 16}" y="{ay + 42}" class="f-agent" font-size="12" font-weight="600">added by your agent · '
          '<tspan font-weight="800" text-decoration="underline">undo</tspan></text>')
    cursor(s, ax + 196, ay + 40, "agent", 1.05)
    s.pill(ax + 210, ay + 54, "agent", "agent", "on", size=10.5, h=19, weight=700, spacing="0.2", pad=8)

    task(s, cols[2], 206, "Review the lab PR", "done 10:12", done=True)
    task(s, cols[2], 274, "Renew parking permit", "done 09:40", done=True)

    # Shortcut keys.
    s.rect(70, 524, 724, 42, 12, fill="panel", stroke="panelb", sw=1.2)
    x = 104
    for key, label in (("A", "add"), ("R", "reject"), ("Z", "undo"), ("E", "edit"), ("/", "ask the agent")):
        x = keycap(s, x, 532, key, label)
    badge(s, 72, 526, 4)

    # Side panel: access switches, then the chat that was always there.
    s.rect(830, 178, 304, 186, 14, fill="card", stroke="you", sw=1.8)
    s.text(848, 205, "Agent access", size=14.5, weight=700)
    access_rows(s, 848, 240, 33, 1046, 1098, 1118, mono=False)
    badge(s, 1132, 180, 5)

    s.rect(830, 376, 304, 194, 14, fill="card", stroke="cardb", sw=1.3)
    s.text(848, 402, "Chat", size=14.5, weight=700)
    s.rect(958, 412, 160, 30, 15, fill="yous", stroke="youb", sw=1.2)
    s.text(1038, 431.5, "Plan my afternoon", fill="ink", size=13, anchor="middle")
    s.rect(846, 450, 232, 50, 15, fill="agents", stroke="agentb", sw=1.2)
    s.text(861, 470.5, "Done. 3 blocks planned and", fill="ink", size=13)
    s.text(861, 488.5, "1 suggestion on the board.", fill="ink", size=13)
    s.rect(846, 512, 272, 42, 21, fill="panel", stroke="cardb", sw=1.3)
    s.text(866, 537.5, "Message your agent…", fill="faint", size=13)
    s.circle(1094, 533, 14, fill="you")
    s.path("M1094,539.5 v-13 m-5.5,5.5 l5.5,-5.5 l5.5,5.5", stroke="on", sw=2.2)
    badge(s, 1132, 378, 1)

    # What the numbers mean.
    items = ["Words: the chat box stays", "Yes / no buttons", "A pen: circle it, write on it", "Shortcut keys",
             "Switches for what it sees and does"]
    widths = [tw(t, 14) + 32 for t in items]
    gap = (ww - sum(widths)) / (len(items) - 1)
    x = wx
    for i, (t, w) in enumerate(zip(items, widths)):
        badge(s, x + 12, 620, i + 1)
        s.text(x + 31, 625, t, fill="ink2", size=14)
        x += w + gap
    return s.render()


# ---------------------------------------------------------------- the ecosystem today (GitHub only)

ECO_STYLE = f"""
  .bg {{ fill: #f7f8fb; }}
  .card {{ fill: #ffffff; stroke: #dfe3eb; stroke-width: 1.2; }}
  .actor {{ fill: #eef2ff; stroke: #c5d0fa; stroke-width: 1.2; }}
  .pill {{ fill: #ffffff; stroke: #b9c4e6; stroke-width: 1.2; }}
  .core {{ fill: #edf1ff; stroke: #3b5bdb; stroke-width: 2; }}
  .base {{ fill: #f0f2f6; stroke: #dfe3eb; stroke-width: 1.2; }}
  .chip {{ fill: #ffffff; stroke: #c5d0fa; stroke-width: 1; }}
  .t {{ fill: #15171c; font-family: {FONT}; }}
  .m {{ fill: #5b6271; font-family: {FONT}; }}
  .mono {{ fill: #3b5bdb; font-family: {MONO}; }}
  .accent {{ fill: #3b5bdb; font-family: {FONT}; }}
  .ok {{ fill: #2b8a3e; font-family: {FONT}; }}
  .dev {{ fill: #c25d00; font-family: {FONT}; }}
  .plug {{ fill: #6741d9; font-family: {FONT}; }}
  .line {{ stroke: #8a93a6; stroke-width: 1.6; fill: none; }}
  .head {{ fill: #8a93a6; }}
  .logo {{ fill: #3b5bdb; }}
  @media (prefers-color-scheme: dark) {{
    .bg {{ fill: #0f1115; }}
    .card {{ fill: #171a21; stroke: #2b303b; }}
    .actor {{ fill: #1a2140; stroke: #34417a; }}
    .pill {{ fill: #171a21; stroke: #3a4466; }}
    .core {{ fill: #161d38; stroke: #7c95f5; }}
    .base {{ fill: #151820; stroke: #2b303b; }}
    .chip {{ fill: #171a21; stroke: #34417a; }}
    .t {{ fill: #e8eaef; }}
    .m {{ fill: #9aa1ad; }}
    .mono {{ fill: #9db0ff; }}
    .accent {{ fill: #8fa5ff; }}
    .ok {{ fill: #63d47a; }}
    .dev {{ fill: #ffa94d; }}
    .plug {{ fill: #b197fc; }}
    .line {{ stroke: #6b7385; }}
    .head {{ fill: #6b7385; }}
    .logo {{ fill: #7c95f5; }}
  }}
"""


class Eco:
    def __init__(self, width, height, title, desc):
        self.width, self.height, self.title, self.desc = width, height, title, desc
        self.parts = []

    def add(self, text):
        self.parts.append(text)

    def rect(self, x, y, w, h, cls, r=14):
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" class="{cls}"/>')

    def text(self, x, y, s, cls="t", size=14, weight=400, anchor="start", spacing=None):
        extra = f' letter-spacing="{spacing}"' if spacing else ""
        self.add(f'<text x="{x}" y="{y}" class="{cls}" font-size="{size}" font-weight="{weight}" text-anchor="{anchor}"{extra}>{escape(s)}</text>')

    def arrow(self, x1, y1, x2, y2):
        self.add(f'<path d="M{x1},{y1} L{x2},{y2 - 7}" class="line" marker-end="url(#arrow)"/>')

    def chips(self, x, y, labels, size=12.5):
        for label in labels:
            width = round(len(label) * size * 0.62 + 26)
            self.rect(x, y, width, 26, "chip", r=13)
            self.text(x + width / 2, y + 17.5, label, "mono", size=size, anchor="middle")
            x += width + 10

    def logo(self, x, y, s=22):
        k = s / 32
        pts = [(9, 22.5), (16, 7.5), (23, 22.5), (16, 18.2)]
        d = "M" + " L".join(f"{x + px * k:.1f},{y + py * k:.1f}" for px, py in pts) + " Z"
        self.add(f'<rect x="{x}" y="{y}" width="{s}" height="{s}" rx="{s * 0.22:.1f}" class="logo"/>')
        self.add(f'<path d="{d}" fill="#ffffff"/>')

    def render(self):
        return (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.width} {self.height}" '
            f'width="{self.width}" height="{self.height}" role="img" aria-labelledby="title desc">\n'
            f'<title id="title">{escape(self.title)}</title>\n<desc id="desc">{escape(self.desc)}</desc>\n'
            f'<style>{ECO_STYLE}</style>\n'
            '<defs><marker id="arrow" viewBox="0 0 10 10" refX="1" refY="5" markerWidth="7" markerHeight="7" '
            'orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" class="head"/></marker></defs>\n'
            f'<rect width="{self.width}" height="{self.height}" rx="24" class="bg"/>\n'
            + "\n".join(self.parts) + "\n</svg>\n")


def fig_ecosystem():
    c1, c2, c3, cw = 60, 435, 810, 330
    m1, m2, m3 = c1 + cw / 2, c2 + cw / 2, c3 + cw / 2
    svg = Eco(1200, 770, "What exists today",
              "You and your agent use the same apps. You work in the browser through LaunchX; your agent reads each "
              "app's JSON state and runs its commands, and agent plugins run inside the agent. LaunchX runs Schedule "
              "Everything and vmc; ppagent runs inside your agent. Everything keeps its data in local files.")

    def card(x, y, h, badge_text, badge_cls, title, lines):
        svg.rect(x, y, cw, h, "card")
        svg.text(x + 22, y + 30, badge_text, badge_cls, size=11.5, weight=700, spacing="0.6")
        svg.text(x + 22, y + 56, title, "t", size=18, weight=650)
        for i, ln in enumerate(lines):
            svg.text(x + 22, y + 82 + i * 20, ln, "m", size=13.5)

    def actor(x, w, title, sub):
        svg.rect(x, 124, w, 80, "actor")
        svg.text(x + 24, 158, title, "t", size=18, weight=650)
        svg.text(x + 24, 184, sub, "m", size=14)

    def pill(x, label):
        svg.rect(x, 252, cw, 48, "pill", r=24)
        svg.text(x + cw / 2, 282, label, "t", size=15, weight=600, anchor="middle")

    svg.logo(60, 42, 30)
    svg.text(102, 66, "What exists today", "t", size=26, weight=700)
    svg.text(60, 100, "Proofs of concept on your own machine: you use each app's page, your agent reads its state and runs its commands.", "m", size=15)

    actor(60, 525, "You", "the page · buttons · your mouse and keyboard")
    actor(615, 525, "Your agent", "Claude Code · Codex · Antigravity")
    pill(c1, "The app's page, in the browser")
    pill(c2, "JSON state + a command for every action")
    pill(c3, "Agent skills & scripts")
    svg.arrow(m1, 204, m1, 252)
    svg.arrow(540, 204, 540, 252)
    svg.arrow(690, 204, 690, 252)
    svg.arrow(m3, 204, m3, 252)

    svg.rect(c1, 348, c2 + cw - c1, 108, "core", r=16)
    svg.logo(c1 + 24, 366, 24)
    svg.text(c1 + 58, 385, "LaunchX", "accent", size=20, weight=700)
    svg.text(c1 + 160, 385, "runs every app · one sign-in · a switch for what agents may use", "m", size=14)
    svg.chips(c1 + 24, 408, ["<app>.localhost:7600", "launchx.toml contract", "lx agents on|off", "LaunchX.app"])
    svg.arrow(m1, 300, m1, 348)
    svg.arrow(m2, 300, m2, 348)
    svg.arrow(m3, 300, m3, 504)
    svg.text(m3 + 14, 405, "runs inside your agent", "m", size=13)

    card(c1, 504, 136, "APP · OPEN STATE", "ok", "Schedule Everything",
         ["Weekly routine, tasks, deadlines and", "focus reminders. Agents suggest tasks;", "you accept or reject them on the board."])
    card(c2, 504, 136, "APP · OPEN STATE · NOT YET RELEASED", "dev", "vmc",
         ["Machine console: live CPU, RAM, GPU", "and disk use of the servers in your", "SSH config."])
    card(c3, 504, 136, "AGENT PLUGIN", "plug", "ppagent",
         ["Reads arXiv papers inside your agent:", "deep reads, daily digests ranked for", "you, and publishing to GitHub Pages."])
    svg.arrow(m1, 456, m1, 504)
    svg.arrow(m2, 456, m2, 504)

    svg.rect(c1, 676, c3 + cw - c1, 58, "base")
    svg.text(600, 711, "Your machine · every app keeps its data in local files you own", "m", size=15, weight=500, anchor="middle")
    return svg.render()


# ---------------------------------------------------------------- command line

def main():
    if len(sys.argv) < 3 or sys.argv[1] not in ("github", "blog"):
        sys.exit(__doc__)
    target, out, only = sys.argv[1], Path(sys.argv[2]), sys.argv[3:]
    out.mkdir(parents=True, exist_ok=True)
    files = {}
    if target == "github":
        files["bottleneck.svg"] = fig_bottleneck("github", "auto")
        files["direct-state.svg"] = fig_direct_state("github", "auto")
        files["shared-interface.svg"] = fig_shared_interface("github", "auto")
        files["ecosystem.svg"] = fig_ecosystem()
    else:
        for mode, suffix in (("light", ""), ("dark", "-dark")):
            files[f"pipeline-then{suffix}.svg"] = fig_pipeline("blog", mode, "then")
            files[f"pipeline-now{suffix}.svg"] = fig_pipeline("blog", mode, "now")
            files[f"direct-state{suffix}.svg"] = fig_direct_state("blog", mode)
            files[f"shared-interface{suffix}.svg"] = fig_shared_interface("blog", mode)
    if only:
        unknown = [n for n in only if not any(f in (f"{n}.svg", f"{n}-dark.svg") for f in files)]
        if unknown:
            sys.exit(f"unknown figure: {', '.join(unknown)}")
        files = {f: body for f, body in files.items() if f.removesuffix(".svg").removesuffix("-dark") in only}
    for name, body in files.items():
        (out / name).write_text(body)
    print("wrote", ", ".join(files), "to", out)


if __name__ == "__main__":
    main()
