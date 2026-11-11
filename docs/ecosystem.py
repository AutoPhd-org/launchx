"""Generate the two AutoPhD ecosystem diagrams as theme-aware SVG."""
import sys
from pathlib import Path
from xml.sax.saxutils import escape

FONT = '-apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif'
MONO = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'

STYLE = f"""
  .bg {{ fill: #f7f8fb; }}
  .card {{ fill: #ffffff; stroke: #dfe3eb; stroke-width: 1.2; }}
  .actor {{ fill: #eef2ff; stroke: #c5d0fa; stroke-width: 1.2; }}
  .pill {{ fill: #ffffff; stroke: #b9c4e6; stroke-width: 1.2; }}
  .core {{ fill: #edf1ff; stroke: #3b5bdb; stroke-width: 2; }}
  .base {{ fill: #f0f2f6; stroke: #dfe3eb; stroke-width: 1.2; }}
  .planned {{ fill: none; stroke: #8a93a6; stroke-width: 1.4; stroke-dasharray: 6 5; }}
  .plannedfill {{ fill: #fbfbfd; stroke: #8a93a6; stroke-width: 1.4; stroke-dasharray: 6 5; }}
  .chip {{ fill: #ffffff; stroke: #c5d0fa; stroke-width: 1; }}
  .t {{ fill: #15171c; font-family: {FONT}; }}
  .m {{ fill: #5b6271; font-family: {FONT}; }}
  .mono {{ fill: #3b5bdb; font-family: {MONO}; }}
  .accent {{ fill: #3b5bdb; font-family: {FONT}; }}
  .ok {{ fill: #2b8a3e; font-family: {FONT}; }}
  .dev {{ fill: #c25d00; font-family: {FONT}; }}
  .plug {{ fill: #6741d9; font-family: {FONT}; }}
  .idea {{ fill: #6b7385; font-family: {FONT}; }}
  .monoidea {{ fill: #6b7385; font-family: {MONO}; }}
  .line {{ stroke: #8a93a6; stroke-width: 1.6; fill: none; }}
  .dline {{ stroke: #8a93a6; stroke-width: 1.6; fill: none; stroke-dasharray: 5 5; }}
  .head {{ fill: #8a93a6; }}
  .logo {{ fill: #3b5bdb; }}
  @media (prefers-color-scheme: dark) {{
    .bg {{ fill: #0f1115; }}
    .card {{ fill: #171a21; stroke: #2b303b; }}
    .actor {{ fill: #1a2140; stroke: #34417a; }}
    .pill {{ fill: #171a21; stroke: #3a4466; }}
    .core {{ fill: #161d38; stroke: #7c95f5; }}
    .base {{ fill: #151820; stroke: #2b303b; }}
    .planned {{ stroke: #6b7385; }}
    .plannedfill {{ fill: #13161c; stroke: #6b7385; }}
    .chip {{ fill: #171a21; stroke: #34417a; }}
    .t {{ fill: #e8eaef; }}
    .m {{ fill: #9aa1ad; }}
    .mono {{ fill: #9db0ff; }}
    .accent {{ fill: #8fa5ff; }}
    .ok {{ fill: #63d47a; }}
    .dev {{ fill: #ffa94d; }}
    .plug {{ fill: #b197fc; }}
    .idea, .monoidea {{ fill: #8d95a6; }}
    .line, .dline {{ stroke: #6b7385; }}
    .head {{ fill: #6b7385; }}
    .logo {{ fill: #7c95f5; }}
  }}
"""


class Svg:
    def __init__(self, width, height, title, desc):
        self.width, self.height = width, height
        self.parts = []
        self.title, self.desc = title, desc

    def add(self, text):
        self.parts.append(text)

    def rect(self, x, y, w, h, cls, r=14):
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" class="{cls}"/>')

    def text(self, x, y, s, cls="t", size=14, weight=400, anchor="start", spacing=None):
        extra = f' letter-spacing="{spacing}"' if spacing else ""
        self.add(f'<text x="{x}" y="{y}" class="{cls}" font-size="{size}" font-weight="{weight}" text-anchor="{anchor}"{extra}>{escape(s)}</text>')

    def arrow(self, x1, y1, x2, y2, dashed=False):
        cls = "dline" if dashed else "line"
        self.add(f'<path d="M{x1},{y1} L{x2},{y2 - 7}" class="{cls}" marker-end="url(#arrow)"/>')

    def chips(self, x, y, labels, planned=False, size=12.5):
        for label in labels:
            width = round(len(label) * size * 0.62 + 26)
            self.rect(x, y, width, 26, "plannedfill" if planned else "chip", r=13)
            self.text(x + width / 2, y + 17.5, label, "monoidea" if planned else "mono", size=size, anchor="middle")
            x += width + 10
        return x

    def logo(self, x, y, s=22):
        k = s / 32
        pts = [(9, 22.5), (16, 7.5), (23, 22.5), (16, 18.2)]
        d = "M" + " L".join(f"{x + px * k:.1f},{y + py * k:.1f}" for px, py in pts) + " Z"
        self.add(f'<rect x="{x}" y="{y}" width="{s}" height="{s}" rx="{s * 0.22:.1f}" class="logo"/>')
        self.add(f'<path d="{d}" fill="#ffffff"/>')

    def render(self):
        head = (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.width} {self.height}" '
            f'width="{self.width}" height="{self.height}" role="img" aria-labelledby="title desc">\n'
            f'<title id="title">{escape(self.title)}</title>\n<desc id="desc">{escape(self.desc)}</desc>\n'
            f'<style>{STYLE}</style>\n'
            '<defs><marker id="arrow" viewBox="0 0 10 10" refX="1" refY="5" markerWidth="7" markerHeight="7" '
            'orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" class="head"/></marker></defs>\n'
            f'<rect width="{self.width}" height="{self.height}" rx="24" class="bg"/>\n'
        )
        return head + "\n".join(self.parts) + "\n</svg>\n"


# Columns shared by both diagrams.
C1, C2, C3, CW = 60, 435, 810, 330
MID1, MID2, MID3 = C1 + CW / 2, C2 + CW / 2, C3 + CW / 2


def card(svg, x, y, w, h, badge, badge_cls, title, lines, cls="card"):
    svg.rect(x, y, w, h, cls)
    svg.text(x + 22, y + 30, badge, badge_cls, size=11.5, weight=700, spacing="0.6")
    svg.text(x + 22, y + 56, title, "t", size=18, weight=650)
    for i, line in enumerate(lines):
        svg.text(x + 22, y + 82 + i * 20, line, "m", size=13.5)


def actor(svg, x, w, y, title, sub):
    svg.rect(x, y, w, 80, "actor")
    svg.text(x + 24, y + 34, title, "t", size=18, weight=650)
    svg.text(x + 24, y + 60, sub, "m", size=14)


def pill(svg, x, y, label, planned=False):
    svg.rect(x, y, CW, 48, "plannedfill" if planned else "pill", r=24)
    svg.text(x + CW / 2, y + 30, label, "idea" if planned else "t", size=15, weight=600, anchor="middle")


def today():
    svg = Svg(1200, 770, "The AutoPhD ecosystem today",
              "You and your coding agent use the same apps. You work in the browser through LaunchX; your agent uses "
              "each app's command line and JSON state, and agent plugins run inside the agent. LaunchX runs Schedule "
              "Everything and vmc; ppagent runs inside your agent. Everything keeps its data in local files.")
    svg.logo(60, 42, 30)
    svg.text(102, 66, "The AutoPhD ecosystem today", "t", size=26, weight=700)
    svg.text(60, 100, "Agent-native apps on your own machine: you use each app in the browser, your agent uses the same app through its CLI.", "m", size=15)

    actor(svg, 60, 525, 124, "You", "LaunchX app · browser tabs · bookmarks")
    actor(svg, 615, 525, 124, "Your coding agent", "Claude Code · Codex · Antigravity")
    pill(svg, C1, 252, "Browser GUI")
    pill(svg, C2, 252, "CLI + JSON state")
    pill(svg, C3, 252, "Agent skills & scripts")
    svg.arrow(MID1, 204, MID1, 252)
    svg.arrow(540, 204, 540, 252)
    svg.arrow(690, 204, 690, 252)
    svg.arrow(MID3, 204, MID3, 252)

    # LaunchX spans the first two columns.
    svg.rect(C1, 348, C2 + CW - C1, 108, "core", r=16)
    svg.logo(C1 + 24, 366, 24)
    svg.text(C1 + 58, 385, "LaunchX", "accent", size=20, weight=700)
    svg.text(C1 + 160, 385, "runs every app · one sign-in · dependencies in order · logs · health", "m", size=14)
    svg.chips(C1 + 24, 408, ["<app>.localhost:7600", "launchx.toml contract", "lx", "LaunchX.app"])
    svg.arrow(MID1, 300, MID1, 348)
    svg.arrow(MID2, 300, MID2, 348)
    svg.arrow(MID3, 300, MID3, 504)
    svg.text(MID3 + 14, 405, "runs inside your agent", "m", size=13)

    card(svg, C1, 504, CW, 136, "SERVICE · LEVEL 2", "ok", "Schedule Everything",
         ["Weekly routine, tasks, deadlines and", "focus reminders. Agents suggest tasks;", "you approve them on the board."])
    card(svg, C2, 504, CW, 136, "SERVICE · LEVEL 2 · NOT YET RELEASED", "dev", "vmc",
         ["Machine console: live CPU, RAM, GPU", "and disk use of the servers in your", "SSH config."])
    card(svg, C3, 504, CW, 136, "AGENT PLUGIN", "plug", "ppagent",
         ["Reads arXiv papers inside your agent:", "deep reads, daily digests ranked for", "you, and publishing to GitHub Pages."])
    svg.arrow(MID1, 456, MID1, 504)
    svg.arrow(MID2, 456, MID2, 504)

    svg.rect(C1, 676, C3 + CW - C1, 58, "base")
    svg.text(600, 711, "Your machine · every app keeps its data in local files you own", "m", size=15, weight=500, anchor="middle")
    return svg.render()


def future():
    svg = Svg(1200, 880, "Where the AutoPhD ecosystem is heading",
              "Planned: one agent connection to every app through an action catalogue served by LaunchX over MCP, "
              "phone and remote access through the gateway, installing apps from a repository, and scheduled jobs. "
              "More apps join: learn-by-quiz and a CV agent are in development, and any open-source app can ship a "
              "launchx.toml. Dashed outlines are plans, not features.")
    svg.logo(60, 42, 30)
    svg.text(102, 66, "Where it's heading", "t", size=26, weight=700)
    svg.text(60, 100, "One launcher, one agent connection, any number of agent-native apps: ours, yours and the community's.", "m", size=15)
    # Legend
    svg.rect(846, 50, 18, 18, "card", r=5)
    svg.text(872, 64, "available", "m", size=13)
    svg.rect(950, 50, 18, 18, "plannedfill", r=5)
    svg.text(976, 64, "planned / idea", "m", size=13)

    actor(svg, 60, 525, 124, "You, anywhere", "LaunchX app · browser · phone")
    actor(svg, 615, 525, 124, "Any agent", "coding agents · MCP clients · scheduled routines")
    pill(svg, C1, 252, "Browser GUI · phone", planned=False)
    pill(svg, C2, 252, "CLI + JSON state")
    pill(svg, C3, 252, "MCP: one action catalogue", planned=True)
    svg.arrow(MID1, 204, MID1, 252)
    svg.arrow(540, 204, 540, 252)
    svg.arrow(690, 204, 690, 252)
    svg.arrow(MID3, 204, MID3, 252, dashed=True)

    svg.rect(C1, 348, C3 + CW - C1, 132, "core", r=16)
    svg.logo(C1 + 24, 366, 24)
    svg.text(C1 + 58, 385, "LaunchX", "accent", size=20, weight=700)
    svg.text(C1 + 160, 385, "the launcher every agent-native app plugs into", "m", size=14)
    svg.text(C1 + 24, 425, "today", "ok", size=12, weight=700, spacing="0.6")
    svg.chips(C1 + 84, 407, ["run & watch", "one sign-in", "dependencies", "logs", "lx check", "remembers what you run"])
    svg.text(C1 + 24, 459, "next", "idea", size=12, weight=700, spacing="0.6")
    svg.chips(C1 + 84, 441, ["lx install <repo>", "phone & remote access", "action catalogue → MCP", "scheduled jobs"], planned=True)
    svg.arrow(MID1, 300, MID1, 348)
    svg.arrow(MID2, 300, MID2, 348)
    svg.arrow(MID3, 300, MID3, 348, dashed=True)

    top = 528
    svg.arrow(MID1, 480, MID1, top)
    svg.arrow(MID2, 480, MID2, top)
    svg.arrow(MID3, 480, MID3, top)
    small = 108
    card(svg, C1, top, CW, small, "AVAILABLE", "ok", "Schedule Everything", ["Routine, tasks and reminders"])
    card(svg, C2, top, CW, small, "SOON", "dev", "vmc", ["Machine console for SSH servers"])
    card(svg, C3, top, CW, small, "OPEN TO ANYONE", "idea", "Your app", ["Any open-source app with a launchx.toml"], cls="plannedfill")
    row2 = top + small + 20
    card(svg, C1, row2, CW, small, "AVAILABLE · AGENT PLUGIN", "plug", "ppagent", ["Paper reading inside your agent"])
    card(svg, C2, row2, CW, small, "IN DEVELOPMENT", "dev", "learn-by-quiz", ["Learn concepts and papers by quiz"])
    card(svg, C3, row2, CW, small, "IN DEVELOPMENT", "dev", "CV agent", ["A CV from your academic record"])

    base = row2 + small + 36
    svg.rect(C1, base, 525, 58, "plannedfill")
    svg.text(C1 + 262, base + 35, "auto-phd.sh · install everything in one go", "idea", size=15, weight=500, anchor="middle")
    svg.rect(615, base, 525, 58, "base")
    svg.text(615 + 262, base + 35, "Your machine · data in local files you own", "m", size=15, weight=500, anchor="middle")
    svg.height = base + 58 + 44
    return svg.render()


out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
(out / "ecosystem-today.svg").write_text(today())
(out / "ecosystem-future.svg").write_text(future())
print("wrote", out)
