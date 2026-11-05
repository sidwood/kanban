"""Shared state and helpers every acceptance test uses."""

import re
import time
from datetime import datetime, timedelta, timezone

from .harness import CoreError

# Tools that only read. Read-only mode refuses every other tool before
# it reaches the core, so a read-only run cannot write by mistake.
READ_TOOLS = frozenset({
    "get_issue",
    "list_issues",
    "list_issue_statuses",
    "list_issue_labels",
    "list_milestones",
    "list_projects",
    "list_comments",
    "comment_revisions",
})

CLOSED_TYPES = frozenset({"completed", "canceled", "duplicate"})
STATE_TYPES = frozenset({"triage", "backlog", "unstarted", "started", "completed", "canceled", "duplicate"})
KEY_PATTERN = re.compile(r"^([A-Z][A-Z0-9]{1,7})-([1-9][0-9]*)$")
TIMESTAMP_PATTERN = re.compile(
    r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})$"
)
DATE_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}$")
COLOUR_PATTERN = re.compile(r"^#[0-9A-Fa-f]{6}$")


class StepError(Exception):
    """A step a test depends on failed, so the test cannot go on."""


def parse_timestamp(text):
    """Parse an RFC 3339 timestamp; `None` when it is not one."""
    if not isinstance(text, str):
        return None
    match = TIMESTAMP_PATTERN.match(text)
    if not match:
        return None
    year, month, day, hour, minute, second, fraction, zone = match.groups()
    micros = int((fraction or "0")[:6].ljust(6, "0"))
    if zone == "Z":
        offset = timezone.utc
    else:
        sign = 1 if zone[0] == "+" else -1
        offset = timezone(sign * timedelta(hours=int(zone[1:3]), minutes=int(zone[4:6])))
    try:
        return datetime(int(year), int(month), int(day), int(hour), int(minute), int(second),
                        micros, tzinfo=offset)
    except ValueError:
        return None


def key_number(key):
    match = KEY_PATTERN.match(key or "")
    return int(match.group(2)) if match else None


def relation_set(record, kind=None):
    """The (kind, identifier) pairs an issue lists, optionally one kind."""
    return {
        (relation["kind"], relation["identifier"])
        for relation in record.get("relations", [])
        if kind is None or relation["kind"] == kind
    }


def has_relation(record, kind, identifier):
    return (kind, identifier) in relation_set(record)


# Fields a write is allowed to move without changing what the issue says.
BOOKKEEPING = ("version", "updated_at")


def changed_fields(before, after, allowed=()):
    """Field names that differ between two records, minus `allowed`."""
    names = set(before) | set(after)
    return sorted(
        name for name in names
        if name not in allowed and before.get(name) != after.get(name)
    )


class Context:
    """One run's clients, recorder, projects, and fixtures."""

    def __init__(self, mcp, socket_client, recorder, run_id, read_only, pause=0.004):
        self.mcp = mcp
        self.socket = socket_client
        self.rec = recorder
        self.run_id = run_id
        self.read_only = read_only
        self.pause = pause
        self.projects = {}
        self.project_code = None
        self.other_code = None
        self.milestones = {}
        self._counter = 0
        self._state_types = {}

    # Identity and mutations ------------------------------------------------

    @property
    def project_id(self):
        return self.projects[self.project_code]

    @property
    def other_project_id(self):
        return self.projects[self.other_code]

    def idempotency_key(self, label):
        self._counter += 1
        return f"acceptance-{self.run_id}-{self._counter:04d}-{label}"

    def mutation(self, version, label="write"):
        return {"optimistic_version": version, "idempotency_key": self.idempotency_key(label)}

    def tick(self):
        """Let the core's millisecond clock move before a timed write."""
        time.sleep(self.pause)

    # MCP -------------------------------------------------------------------

    def call(self, tool, arguments):
        if self.read_only and tool not in READ_TOOLS:
            raise StepError(f"read-only mode refuses the write tool {tool}")
        return self.mcp.call(tool, arguments)

    def must(self, tool, arguments, what=None):
        result = self.call(tool, arguments)
        if not result.ok:
            raise StepError(f"{what or tool} was refused: {self.rec.describe_result(result)}")
        return result.payload

    def get(self, key):
        return self.must("get_issue", {"identifier": key}, f"get_issue {key}")

    def version(self, key):
        return self.get(key)["version"]

    def create(self, title, project=None, **fields):
        """Create an issue; `project` is a code, an id, or None for none."""
        arguments = {"mutation": self.mutation(0, "create"), "title": title}
        if project is not None:
            arguments["project_id"] = self.projects.get(project, project)
        arguments.update(fields)
        self.tick()
        return self.must("save_issue", arguments, f"create {title!r}")["identifier"]

    def save(self, key, version=None, **fields):
        """Update an issue at its current version and return the result."""
        if version is None:
            version = self.version(key)
        arguments = {"mutation": self.mutation(version, "update"), "identifier": key}
        arguments.update(fields)
        self.tick()
        return self.call("save_issue", arguments)

    def save_must(self, key, **fields):
        result = self.save(key, **fields)
        if not result.ok:
            raise StepError(
                f"save_issue {key} {sorted(fields)} was refused: {self.rec.describe_result(result)}"
            )
        return result.payload

    def list_page(self, **arguments):
        return self.must("list_issues", arguments, "list_issues")

    def list_all(self, limit=50, **filters):
        """Every issue a filter matches, following `next_cursor`."""
        issues, pages = self.list_pages(limit, **filters)
        return issues

    def list_pages(self, limit, **filters):
        issues = []
        pages = []
        cursor = None
        while True:
            arguments = dict(filters)
            arguments["limit"] = limit
            if cursor:
                arguments["cursor"] = cursor
            page = self.list_page(**arguments)
            pages.append(page)
            issues.extend(page.get("issues", []))
            cursor = page.get("next_cursor")
            if not cursor:
                return issues, pages
            if len(pages) > 5000:
                raise StepError("list_issues kept returning a next_cursor")

    def state_types(self, project_id):
        """Workflow state name to type for one project."""
        if project_id not in self._state_types:
            states = self.must("list_issue_statuses", {"project_id": project_id})["states"]
            self._state_types[project_id] = {state["name"]: state["state_type"] for state in states}
        return self._state_types[project_id]

    # Socket ----------------------------------------------------------------

    def legacy(self, operation, payload):
        """Send a legacy core command over the socket; refusals raise."""
        if self.read_only or self.socket is None:
            raise StepError(f"read-only mode refuses the legacy command {operation}")
        self.tick()
        try:
            return self.socket.command(operation, payload)
        except CoreError as error:
            shown = error.error if self.rec is None else self.rec.redact_value(error.error)
            text = f"{operation} was refused: {shown}"
            if self.rec is not None:
                text = self.rec.redact_text(text)
            raise StepError(text)

    # Shared checks ---------------------------------------------------------

    def refused(self, description, result, code, field=None):
        """A refusal carrying the expected code and, when given, field."""
        if result.ok:
            return self.rec.check(description, False, "expected a refusal, the call succeeded")
        wanted = f"{code}" + (f" field={field}" if field else "")
        ok = result.code == code and (field is None or result.field == field)
        shown = self.rec.describe_result(result)
        reason = shown if ok else f"wanted {wanted}; got {shown}"
        return self.rec.check(description, ok, reason)

    def unchanged(self, description, key, before):
        """The refused write left the issue exactly as it was."""
        after = self.get(key)
        differences = changed_fields(before, after)
        if differences:
            return self.rec.check(description, False, "changed: " + ", ".join(differences))
        return self.rec.check(description, True, f"version still {after['version']}")

    def only_changed(self, description, before, after, allowed):
        """A write moved exactly the fields it named (plus bookkeeping)."""
        differences = changed_fields(before, after, tuple(allowed) + BOOKKEEPING)
        if differences:
            return self.rec.check(description, False, "also changed: " + ", ".join(differences))
        return self.rec.check(description, True, "nothing else moved")
