"""Regenerate the skills' references/ folders from the Tauri v2 docs source.

Sparse-clones tauri-apps/tauri-docs (the MDX behind https://v2.tauri.app: concept, security,
develop, distribute, plugin) and tauri-apps/plugins-workspace (each plugin's generated permission
reference) into a temp dir, flattens the MDX into Markdown and rewrites every skills/*/references/.
SKILL.md files are not touched; review them against the diff of references/ after a sync.

Usage (from anywhere): py -I .claude/skills/tauri-dev/scripts/sync_docs.py
"""
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import date
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "skills"
SKILLS = ["tauri-architecture", "tauri-security", "tauri-ipc", "tauri-develop", "tauri-testing",
          "tauri-plugin-development", "tauri-official-plugins", "tauri-distribute"]
DOCS = PERMS = None
COMMIT = DATE = ""
SITE = "https://v2.tauri.app"
SECTIONS = ["concept", "security", "develop", "distribute", "plugin"]

FENCE = re.compile(r"^\s*(`{3,}|~{3,})")


def skill_for(rel: Path) -> str | None:
    parts = rel.parts
    top = parts[0]
    if top == "concept":
        return "tauri-architecture"
    if top == "security":
        return "tauri-security"
    if top == "distribute":
        return "tauri-distribute"
    if top == "plugin":
        return "tauri-official-plugins"
    # develop/
    if parts[1] == "_sections":
        return None  # partials are inlined
    if parts[1] in ("calling-rust.mdx", "calling-frontend.mdx", "state-management.mdx"):
        return "tauri-ipc"
    if parts[1] == "Tests":
        return "tauri-testing"
    if parts[1] == "Plugins":
        return "tauri-plugin-development"
    return "tauri-develop"


def slug(s: str) -> str:
    return s.lower().replace(" ", "-")


def url_for(rel: Path) -> str:
    parts = [slug(p) for p in rel.with_suffix("").parts]
    if parts[-1] == "index":
        parts = parts[:-1]
    return f"{SITE}/{'/'.join(parts)}/"


def ref_name(rel: Path) -> str:
    parts = [slug(p) for p in rel.with_suffix("").parts[1:]]
    if parts and parts[-1] == "index":
        parts = parts[:-1]
    return ("-".join(parts) or "overview") + ".md"


def split_frontmatter(text: str) -> tuple[dict, str]:
    fm = {}
    if text.startswith("---"):
        end = text.find("\n---", 3)
        raw, text = text[3:end], text[end + 4 :]
        for line in raw.splitlines():
            m = re.match(r"^(\w+):\s*(.*)$", line)
            if m:
                fm[m.group(1)] = m.group(2).strip().strip("'\"")
    return fm, text


def segments(body: str):
    """Yield (is_code, text) chunks, keeping fenced code blocks untouched."""
    buf, code, marker = [], False, None
    for line in body.splitlines(keepends=True):
        m = FENCE.match(line)
        if not code and m:
            if buf:
                yield False, "".join(buf)
            buf, code, marker = [line], True, m.group(1)
        elif code and m and m.group(1)[0] == marker[0] and len(m.group(1)) >= len(marker) \
                and line.strip() == m.group(1):
            buf.append(line)
            yield True, "".join(buf)
            buf, code = [], False
        else:
            buf.append(line)
    if buf:
        yield code, "".join(buf)


def attrs(s: str) -> dict:
    out = {}
    for m in re.finditer(r"(\w+)\s*=\s*(?:\"([^\"]*)\"|'([^']*)'|\{`([^`]*)`\}|\{\"([^\"]*)\"\})", s):
        out[m.group(1)] = next(g for g in m.groups()[1:] if g is not None)
    return out


def abs_link(href: str, page_url: str) -> str:
    if href.startswith("/"):
        return SITE + href
    if href.startswith("./") or href.startswith("../"):
        base = page_url
        for part in href.split("/"):
            if part == "..":
                base = base.rstrip("/").rsplit("/", 1)[0] + "/"
            elif part not in (".", ""):
                base += part + "/" if "#" not in part and "." not in part else part
        return base
    return href


def command_tabs(m: re.Match) -> str:
    a = attrs(m.group(1))
    lines = []
    for pm, cmd in a.items():
        lines.append(f"# {pm}\n{cmd.strip()}")
    return "\n```sh\n" + "\n".join(lines) + "\n```\n"


def link_card(m: re.Match) -> str:
    a = attrs(m.group(1))
    desc = f": {a['description']}" if a.get("description") else ""
    return f"- [{a.get('title', a.get('href'))}]({a.get('href', '')}){desc}"


def transform_prose(text: str, ctx: dict) -> str:
    # MDX imports (top-level only; code samples are in fenced blocks and untouched)
    text = re.sub(r"^import\s[\s\S]*?\sfrom\s+['\"][^'\"]+['\"];?[ \t]*\n", "", text, flags=re.M)
    text = re.sub(r"^import\s+['\"][^'\"]+['\"];?[ \t]*\n", "", text, flags=re.M)
    text = re.sub(r"<FrontendListen\s*/>", lambda _: ctx["partials"]["frontend-listen"], text)
    text = re.sub(r"<CommandTabs([\s\S]*?)/>", command_tabs, text)
    text = re.sub(r"<TabItem\s+([^>]*)>", lambda m: f"\n**{attrs(m.group(1)).get('label', 'Tab')}:**\n", text)
    text = re.sub(r"<LinkCard\s+([\s\S]*?)/>", link_card, text)
    text = re.sub(r"<LinkButton\s+([^>]*)>([\s\S]*?)</LinkButton>",
                  lambda m: f"[{m.group(2).strip()}]({attrs(m.group(1)).get('href', '')})", text)
    text = re.sub(r"<SinceVersion\s+([^>]*)/>", lambda m: f"*(since v{attrs(m.group(1)).get('version', '?')})*", text)
    text = re.sub(r"<PluginPermissions[^>]*/>",
                  "See **Permission reference** at the end of this file (generated from the plugin's "
                  "`permissions/autogenerated/reference.md`).", text)
    text = re.sub(r"<Compatibility[^>]*/>",
                  f"> Platform support table is generated on the site: {ctx['url']}", text)
    text = re.sub(r"<(FeaturesList|TableCompatibility)[^>]*/>",
                  f"> (Generated table on the site: {ctx['url']})", text)
    text = re.sub(r"<Code\s+([^>]*)/>",
                  lambda m: f"\n```{attrs(m.group(1)).get('lang', '')}\n{attrs(m.group(1)).get('code', '')}\n```\n", text)
    text = re.sub(r"<(Image|PluginLinks|AwesomeTauri)\b[\s\S]*?/>", "", text)
    text = re.sub(r"</?(Tabs|TabItem|Steps|CardGrid|ShowSolution|Fragment|Search)\b[^>]*>", "", text)
    # links -> absolute site URLs
    text = re.sub(r"\]\(([^)\s]+)\)", lambda m: f"]({abs_link(m.group(1), ctx['url'])})", text)
    text = re.sub(r"href=\"(/[^\"]*)\"", lambda m: f'href="{SITE}{m.group(1)}"', text)
    return text


def convert(path: Path, rel: Path, partials: dict) -> tuple[str, dict]:
    fm, body = split_frontmatter(path.read_text(encoding="utf-8"))
    ctx = {"url": url_for(rel), "partials": partials}
    out = []
    for is_code, chunk in segments(body):
        out.append(chunk if is_code else transform_prose(chunk, ctx))
    text = "".join(out)
    text = re.sub(r"\n[ \t]*\n(?:[ \t]*\n)+", "\n\n", text).strip() + "\n"
    header = f"# {fm.get('title', rel.stem)}\n\n> Source: {ctx['url']} (tauri-docs `{COMMIT[:7]}`, {DATE}). " \
             f"Local copy; site components flattened, links point to the live site.\n"
    if fm.get("description"):
        header += f">\n> {fm['description']}\n"
    plugin = fm.get("plugin")
    if plugin:
        perm = PERMS / plugin / "permissions" / "autogenerated" / "reference.md"
        if perm.exists():
            text += "\n## Permission reference\n\n> Source: tauri-apps/plugins-workspace " \
                    f"`plugins/{plugin}/permissions/autogenerated/reference.md`\n\n" + perm.read_text(encoding="utf-8").strip() + "\n"
        else:
            text += f"\n## Permission reference\n\nThis plugin ships no permission reference in plugins-workspace.\n"
    return header + "\n" + text, {"title": fm.get("title", rel.stem), "url": ctx["url"], "plugin": plugin}


def git(*args: str, cwd: Path | None = None) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def clone(tmp: Path) -> None:
    global DOCS, PERMS, COMMIT, DATE
    docs_repo, pw_repo = tmp / "tauri-docs", tmp / "plugins-workspace"
    git("clone", "--depth", "1", "--filter=blob:none", "--sparse",
        "https://github.com/tauri-apps/tauri-docs.git", str(docs_repo))
    git("sparse-checkout", "set", *(f"src/content/docs/{s}" for s in SECTIONS), cwd=docs_repo)
    git("clone", "--depth", "1", "--filter=blob:none", "--no-checkout",
        "https://github.com/tauri-apps/plugins-workspace.git", str(pw_repo))
    git("sparse-checkout", "set", "--no-cone", "/plugins/*/permissions/autogenerated/reference.md", cwd=pw_repo)
    git("checkout", cwd=pw_repo)
    DOCS = docs_repo / "src" / "content" / "docs"
    PERMS = pw_repo / "plugins"
    COMMIT = git("rev-parse", "HEAD", cwd=docs_repo)
    DATE = date.today().isoformat()


def main():
    tmp = Path(tempfile.mkdtemp(prefix="tauri-docs-"))
    try:
        clone(tmp)
        for skill in SKILLS:
            shutil.rmtree(OUT / skill / "references", ignore_errors=True)
        convert_all()
        print(f"\nreferences regenerated from tauri-docs {COMMIT[:7]} ({DATE})", file=sys.stderr)
    finally:
        # git marks pack files read-only, which blocks deletion on Windows
        shutil.rmtree(tmp, onexc=lambda fn, p, _: (Path(p).chmod(0o700), fn(p)))


def convert_all():
    partials = {}
    for p in (DOCS / "develop" / "_sections").glob("*.mdx"):
        fm, body = split_frontmatter(p.read_text(encoding="utf-8"))
        body = re.sub(r"^import\s[\s\S]*?\sfrom\s+['\"][^'\"]+['\"];?[ \t]*\n", "", body, flags=re.M)
        partials[p.stem] = body
    index = {}
    for section in SECTIONS:
        for path in sorted((DOCS / section).rglob("*.md*")):
            rel = path.relative_to(DOCS)
            skill = skill_for(rel)
            if not skill:
                continue
            text, meta = convert(path, rel, partials)
            dest = OUT / skill / "references" / ref_name(rel)
            dest.parent.mkdir(parents=True, exist_ok=True)
            if dest.exists():
                raise SystemExit(f"name clash: {dest}")
            dest.write_text(text, encoding="utf-8", newline="\n")
            index.setdefault(skill, []).append((dest.name, meta))
    for skill, items in index.items():
        print(f"\n## {skill}")
        for name, meta in items:
            print(f"{name}\t{meta['title']}\t{meta['url']}")


if __name__ == "__main__":
    main()
