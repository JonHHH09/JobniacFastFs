#!/usr/bin/env python3
"""Change classifier of the CI and E2E workflows (called by scripts/ci/gate.sh).

Reads changed paths, gives each the classes of the first matching rule of change_classes.toml (no
match: `unknown`), and turns the classes, the workflow and the release-call scope into the jobs that
run. Standard library only; run it with -I (the script directory is not on sys.path), so the rule file
is found by path.

    git diff --name-only -z HEAD^1 HEAD | python3 -I scripts/ci/classify.py --workflow ci -z
    python3 -I scripts/ci/classify.py --workflow e2e --scope linux --all

Output, one line each (read by gate.sh):

    classes <a,b,c>             the classes found ("-" when there is none)
    unknown <path>              one line per path no rule matched
    job <name> <state> <by>     state: run | no-class | out-of-scope; by: the classes (or `always`) that
                                switch the job on, "-" when none

Exit status: 0 ok, 2 bad usage, bad scope or bad rule file.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

RULES_FILE = Path(__file__).resolve().with_name("change_classes.toml")
UNKNOWN = "unknown"


class RuleError(ValueError):
    """The rule file or an argument is invalid."""


def glob_to_regex(glob: str) -> re.Pattern[str]:
    """`*` and `?` stay inside one path segment, `**` (a whole segment) crosses `/`.

    `**/x` also matches `x` at the root. There are no character classes: `[` is a literal.
    """
    out: list[str] = []
    i = 0
    while i < len(glob):
        if glob.startswith("**", i):
            starts_segment = i == 0 or glob[i - 1] == "/"
            ends_segment = i + 2 == len(glob) or glob[i + 2] == "/"
            if not (starts_segment and ends_segment):
                raise RuleError(f"`**` must be a whole path segment in glob {glob!r}")
            if glob.startswith("**/", i):
                out.append("(?:.*/)?")
                i += 3
            else:
                out.append(".*")
                i += 2
        elif glob[i] == "*":
            out.append("[^/]*")
            i += 1
        elif glob[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(glob[i]))
            i += 1
    return re.compile("".join(out), re.DOTALL)  # DOTALL: `**` crosses a newline in a name like `*` does


@dataclass(frozen=True)
class Rule:
    globs: tuple[re.Pattern[str], ...]
    classes: tuple[str, ...]

    def matches(self, path: str) -> bool:
        return any(pattern.fullmatch(path) for pattern in self.globs)


@dataclass(frozen=True)
class Table:
    rules: tuple[Rule, ...]
    class_jobs: dict[str, tuple[str, ...]]
    workflows: dict[str, tuple[str, ...]]
    always: tuple[str, ...]
    scopes: dict[str, tuple[str, ...]]

    def all_jobs(self) -> frozenset[str]:
        return frozenset(job for jobs in self.workflows.values() for job in jobs)

    def expand(self, entries: tuple[str, ...]) -> frozenset[str]:
        """Job names of a class or scope entry list; `@ci`, `@e2e` and `@all` name whole sets."""
        jobs: set[str] = set()
        for entry in entries:
            if entry == "@all":
                jobs |= self.all_jobs()
            elif entry.startswith("@"):
                name = entry[1:]
                if name not in self.workflows:
                    raise RuleError(f"unknown job set {entry!r}")
                jobs |= set(self.workflows[name])
            else:
                if entry not in self.all_jobs():
                    raise RuleError(f"unknown job {entry!r}")
                jobs.add(entry)
        return frozenset(jobs)


def load_table(path: Path = RULES_FILE) -> Table:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    class_jobs = {name: tuple(jobs) for name, jobs in data["classes"].items()}
    rules = []
    for raw in data["rule"]:
        classes = tuple(raw["classes"])
        for name in classes:
            if name not in class_jobs:
                raise RuleError(f"rule {raw['globs']!r} names the undefined class {name!r}")
        rules.append(Rule(tuple(glob_to_regex(glob) for glob in raw["globs"]), classes))
    table = Table(
        rules=tuple(rules),
        class_jobs=class_jobs,
        workflows={name: tuple(jobs) for name, jobs in data["workflows"].items()},
        always=tuple(data["always"]["jobs"]),
        scopes={name: tuple(jobs) for name, jobs in data["scopes"].items()},
    )
    for entries in (*class_jobs.values(), *table.scopes.values(), table.always):  # validates every job name
        table.expand(entries)
    if UNKNOWN not in class_jobs or "all" not in table.scopes:
        raise RuleError("the rule file must define the class `unknown` and the scope `all`")
    return table


def classify_path(table: Table, path: str) -> tuple[str, ...]:
    for rule in table.rules:
        if rule.matches(path):
            return rule.classes
    return (UNKNOWN,)


def classify(table: Table, paths: list[str]) -> tuple[set[str], list[str]]:
    """The classes of all paths and the paths that matched no rule."""
    classes: set[str] = set()
    unknown: list[str] = []
    for path in paths:
        found = classify_path(table, path)
        classes.update(found)
        if found == (UNKNOWN,):
            unknown.append(path)
    return classes, unknown


def jobs_for(table: Table, classes: set[str], workflow: str, scope: str) -> dict[str, tuple[str, list[str]]]:
    """For each job of the workflow: (state, classes that switch it on)."""
    if workflow not in table.workflows:
        raise RuleError(f"unknown workflow {workflow!r}")
    scope = scope or "all"
    if scope not in table.scopes:
        raise RuleError(f"invalid scope {scope!r}; allowed: {', '.join(table.scopes)}")
    in_scope = table.expand(table.scopes[scope])
    result: dict[str, tuple[str, list[str]]] = {}
    for job in table.workflows[workflow]:
        by = sorted(name for name in classes if job in table.expand(table.class_jobs[name]))
        if job in table.always:
            by.append("always")
        if job not in in_scope:
            state = "out-of-scope"
        elif by:
            state = "run"
        else:
            state = "no-class"
        result[job] = (state, by)
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--workflow", required=True, help="ci or e2e")
    parser.add_argument("--scope", default="", help="release-call scope; empty means all")
    parser.add_argument("--rules", type=Path, default=RULES_FILE, help="rule file (default: next to this script)")
    parser.add_argument("--all", action="store_true", help="do not read paths: act as if the class `all` was found")
    parser.add_argument("--unknown", action="store_true", help="do not read paths: act as if one path matched no rule")
    parser.add_argument("-z", action="store_true", help="paths on stdin are NUL separated (default: newline)")
    args = parser.parse_args(argv)
    try:
        table = load_table(args.rules)
        if args.all:
            classes = {"all"}
            unknown: list[str] = []
        elif args.unknown:
            classes = {UNKNOWN}
            unknown = []
        else:
            raw = sys.stdin.buffer.read().decode("utf-8", errors="surrogateescape")
            paths = [p for p in raw.split("\0" if args.z else "\n") if p]
            classes, unknown = classify(table, paths)
        jobs = jobs_for(table, classes, args.workflow, args.scope)
    except RuleError as error:
        print(f"classify: {error}", file=sys.stderr)
        return 2
    print("classes", ",".join(sorted(classes)) or "-")
    for path in unknown:
        print("unknown", path)
    for job, (state, by) in jobs.items():
        print("job", job, state, ",".join(by) or "-")
    return 0


if __name__ == "__main__":
    sys.stdout.reconfigure(newline="\n")  # type: ignore[union-attr]  # no CRLF for bash on Windows
    sys.exit(main())
