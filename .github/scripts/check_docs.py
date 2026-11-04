"""Check this documentation-only repository without building the future app."""

from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[2]
SPEC = ROOT / "docs/specs/native-agent-workbench-mvp.md"
errors = []
spec = SPEC.read_text(encoding="utf-8")
expected_headings = ["想实现什么", "为什么选择这个方案", "我想要什么", "我不想要什么"]
if re.findall(r"^## (.+)$", spec, re.MULTILINE) != expected_headings:
    errors.append("Spec must use the four to-spec sections in order")

definitions = re.findall(r"\*\*((?:R|N|AC)-\d+)\*\*", spec)
if len(definitions) != len(set(definitions)):
    errors.append("Duplicate requirement or acceptance ID")
for prefix in ("R-", "N-", "AC-"):
    if not any(item.startswith(prefix) for item in definitions):
        errors.append(f"Missing {prefix} definitions")
for reference in set(re.findall(r"\b(?:R|N|AC)-\d+\b", spec)):
    if reference not in definitions:
        errors.append(f"Undefined Spec reference: {reference}")

files = [ROOT / name for name in ("README.md", "AGENTS.md", "CONTRIBUTING.md")]
files += sorted((ROOT / "docs").rglob("*.md"))
files += sorted((ROOT / ".github").rglob("*.md"))
for path in files:
    text = path.read_text(encoding="utf-8")
    for target in re.findall(r"\[[^\]\n]+\]\(([^\s)]+)\)", text):
        parsed = urlsplit(target.strip("<>"))
        if parsed.scheme or parsed.netloc or not parsed.path:
            continue
        resolved = (path.parent / unquote(parsed.path)).resolve()
        if not resolved.is_relative_to(ROOT) or not resolved.exists():
            errors.append(f"{path.relative_to(ROOT)}: broken local link {target}")
    if re.search(r"[A-Za-z]:[\\/]Users[\\/]", text):
        errors.append(f"{path.relative_to(ROOT)}: machine-local user path")

if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print(f"OK: {len(files)} Markdown files; {len(definitions)} requirement/acceptance IDs")
